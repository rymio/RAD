//! Terminal sessions (local shell or SSH) on a pseudo-terminal, shown with xterm.js. Sessions
//! live here, not in the page, so they keep running — and keep their scrollback — while the
//! user looks at something else.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter, State};

use crate::client::{CmdError, CmdResult};

/// Scrollback replayed when a view re-attaches.
const HISTORY: usize = 512 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct TermInfo {
    pub id: u32,
    pub title: String,
    pub kind: String,
    pub exited: Option<String>,
}

struct Session {
    info: Mutex<TermInfo>,
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
    history: Mutex<VecDeque<u8>>,
    out: Mutex<Option<Channel<InvokeResponseBody>>>,
}

#[derive(Default)]
pub struct Terminals {
    sessions: Mutex<HashMap<u32, Arc<Session>>>,
    next: AtomicU32,
}

impl Terminals {
    fn get(&self, id: u32) -> CmdResult<Arc<Session>> {
        self.sessions.lock().unwrap().get(&id).cloned().ok_or_else(|| CmdError::new("not_found", "That terminal is closed"))
    }

    pub fn close_all(&self) {
        for (_, s) in self.sessions.lock().unwrap().drain() {
            let _ = s.child.lock().unwrap().kill();
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct OpenArgs {
    /// "local" or "ssh".
    kind: String,
    host: Option<String>,
    user: Option<String>,
    port: Option<u16>,
    identity_file: Option<String>,
    cols: u16,
    rows: u16,
}

fn host_ok(h: &str) -> bool {
    !h.is_empty() && h.len() <= 253 && !h.starts_with('-') && h.chars().all(|c| c.is_ascii_alphanumeric() || ".-_:[]".contains(c))
}

fn user_ok(u: &str) -> bool {
    !u.is_empty() && u.len() <= 64 && !u.starts_with('-') && u.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

fn home() -> Option<std::path::PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(Into::into)
}

fn command(a: &OpenArgs) -> CmdResult<(CommandBuilder, String)> {
    match a.kind.as_str() {
        "ssh" => {
            let host = a.host.as_deref().map(str::trim).filter(|h| host_ok(h)).ok_or_else(|| CmdError::new("bad_request", "Enter a valid host name"))?;
            let user = a.user.as_deref().map(str::trim).filter(|u| !u.is_empty());
            if user.is_some_and(|u| !user_ok(u)) {
                return Err(CmdError::new("bad_request", "That user name isn't valid"));
            }
            let mut cmd = CommandBuilder::new("ssh");
            if let Some(p) = a.port.filter(|p| *p != 22) {
                cmd.args(["-p", &p.to_string()]);
            }
            if let Some(k) = a.identity_file.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
                let k = match (k.strip_prefix("~/"), home()) {
                    (Some(rest), Some(h)) => h.join(rest).display().to_string(),
                    _ => k.to_string(),
                };
                cmd.args(["-i", &k]);
            }
            let target = match user {
                Some(u) => format!("{u}@{host}"),
                None => host.to_string(),
            };
            // `--` ends options: nothing after it can be read as an ssh flag.
            cmd.args(["--", &target]);
            Ok((cmd, target))
        }
        _ => {
            let shell = if cfg!(windows) {
                std::env::var("COMSPEC").unwrap_or_else(|_| "powershell.exe".into())
            } else {
                std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into())
            };
            let mut cmd = CommandBuilder::new(&shell);
            if !cfg!(windows) {
                cmd.arg("-l");
            }
            let title = std::path::Path::new(&shell).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(shell.clone());
            Ok((cmd, title))
        }
    }
}

#[tauri::command]
pub fn term_open(app: AppHandle, t: State<'_, Terminals>, args: OpenArgs) -> CmdResult<TermInfo> {
    let (mut cmd, title) = command(&args)?;
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    if let Some(h) = home() {
        cmd.cwd(h);
    }
    let pty = native_pty_system()
        .openpty(PtySize { rows: args.rows.max(2), cols: args.cols.max(10), pixel_width: 0, pixel_height: 0 })
        .map_err(|e| CmdError::new("terminal", e.to_string()))?;
    let child = pty.slave.spawn_command(cmd).map_err(|e| CmdError::new("terminal", format!("Couldn't start {title}: {e}")))?;
    drop(pty.slave);
    let mut reader = pty.master.try_clone_reader().map_err(|e| CmdError::new("terminal", e.to_string()))?;
    let writer = pty.master.take_writer().map_err(|e| CmdError::new("terminal", e.to_string()))?;
    let id = t.next.fetch_add(1, Ordering::Relaxed) + 1;
    let info = TermInfo { id, title, kind: args.kind.clone(), exited: None };
    let s = Arc::new(Session {
        info: Mutex::new(info.clone()),
        writer: Mutex::new(writer),
        master: Mutex::new(pty.master),
        child: Mutex::new(child),
        history: Mutex::new(VecDeque::new()),
        out: Mutex::new(None),
    });
    t.sessions.lock().unwrap().insert(id, s.clone());
    std::thread::spawn(move || {
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    {
                        let mut h = s.history.lock().unwrap();
                        h.extend(&buf[..n]);
                        let extra = h.len().saturating_sub(HISTORY);
                        h.drain(..extra);
                    }
                    if let Some(ch) = s.out.lock().unwrap().as_ref() {
                        let _ = ch.send(InvokeResponseBody::Raw(buf[..n].to_vec()));
                    }
                }
            }
        }
        let status = s.child.lock().unwrap().wait().map(|st| if st.success() { "exited".to_string() } else { format!("exited with code {}", st.exit_code()) });
        let msg = status.unwrap_or_else(|e| e.to_string());
        s.info.lock().unwrap().exited = Some(msg.clone());
        if let Some(ch) = s.out.lock().unwrap().as_ref() {
            let _ = ch.send(InvokeResponseBody::Json(json!({ "exit": msg }).to_string()));
        }
        let _ = app.emit_to("main", "term-changed", ());
    });
    Ok(info)
}

#[tauri::command]
pub fn term_list(t: State<'_, Terminals>) -> Vec<TermInfo> {
    let mut v: Vec<TermInfo> = t.sessions.lock().unwrap().values().map(|s| s.info.lock().unwrap().clone()).collect();
    v.sort_by_key(|i| i.id);
    v
}

/// Connects a view: replays the scrollback, then streams new output to `on_data`.
#[tauri::command]
pub fn term_attach(t: State<'_, Terminals>, id: u32, on_data: Channel<InvokeResponseBody>) -> CmdResult<()> {
    let s = t.get(id)?;
    let history: Vec<u8> = s.history.lock().unwrap().iter().copied().collect();
    let mut out = s.out.lock().unwrap();
    if !history.is_empty() {
        let _ = on_data.send(InvokeResponseBody::Raw(history));
    }
    if let Some(e) = &s.info.lock().unwrap().exited {
        let _ = on_data.send(InvokeResponseBody::Json(json!({ "exit": e }).to_string()));
    }
    *out = Some(on_data);
    Ok(())
}

#[tauri::command]
pub fn term_detach(t: State<'_, Terminals>, id: u32) {
    if let Ok(s) = t.get(id) {
        *s.out.lock().unwrap() = None;
    }
}

#[tauri::command]
pub fn term_write(t: State<'_, Terminals>, id: u32, data: String) -> CmdResult<()> {
    let s = t.get(id)?;
    let mut w = s.writer.lock().unwrap();
    w.write_all(data.as_bytes()).and_then(|_| w.flush()).map_err(|e| CmdError::new("terminal", e.to_string()))
}

#[tauri::command]
pub fn term_resize(t: State<'_, Terminals>, id: u32, cols: u16, rows: u16) -> CmdResult<()> {
    let s = t.get(id)?;
    let r = s.master.lock().unwrap().resize(PtySize { rows: rows.max(2), cols: cols.max(10), pixel_width: 0, pixel_height: 0 });
    r.map_err(|e| CmdError::new("terminal", e.to_string()))
}

#[tauri::command]
pub fn term_close(app: AppHandle, t: State<'_, Terminals>, id: u32) {
    if let Some(s) = t.sessions.lock().unwrap().remove(&id) {
        let _ = s.child.lock().unwrap().kill();
    }
    let _ = app.emit_to("main", "term-changed", ());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(host: &str, user: &str, port: u16, key: &str) -> OpenArgs {
        OpenArgs { kind: "ssh".into(), host: Some(host.into()), user: Some(user.into()), port: Some(port), identity_file: Some(key.into()), cols: 80, rows: 24 }
    }

    #[test]
    fn ssh_command_is_safe() {
        let (cmd, title) = command(&args("coursemat.au", "deploy", 2222, "/k/id")).unwrap();
        let argv: Vec<String> = cmd.get_argv().iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(argv, ["ssh", "-p", "2222", "-i", "/k/id", "--", "deploy@coursemat.au"]);
        assert_eq!(title, "deploy@coursemat.au");
        assert!(command(&args("-oProxyCommand=evil", "", 22, "")).is_err());
        assert!(command(&args("host.example", "$(id)", 22, "")).is_err());
        assert!(command(&args("a.b;rm -rf ~", "", 22, "")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn pty_runs_a_program_and_reads_its_output() {
        let pty = native_pty_system().openpty(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 }).unwrap();
        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.args(["-c", "printf 'cols=%s\\n' \"$(tput cols 2>/dev/null || echo 80)\""]);
        cmd.env("TERM", "xterm-256color");
        let mut child = pty.slave.spawn_command(cmd).unwrap();
        drop(pty.slave);
        let mut r = pty.master.try_clone_reader().unwrap();
        let mut out = String::new();
        let mut buf = [0u8; 1024];
        while let Ok(n) = r.read(&mut buf) {
            if n == 0 {
                break;
            }
            out.push_str(&String::from_utf8_lossy(&buf[..n]));
            if out.contains('\n') {
                break;
            }
        }
        child.wait().unwrap();
        assert!(out.contains("cols=80"), "{out:?}");
    }
}
