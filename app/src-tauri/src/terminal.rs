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
    /// The Linux sandbox this session runs in.
    pub sandbox: Option<String>,
}

struct Session {
    info: Mutex<TermInfo>,
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
    history: Mutex<VecDeque<u8>>,
    out: Mutex<Option<Channel<InvokeResponseBody>>>,
    /// A sandbox's private network; stopped with the session.
    net: Mutex<Option<crate::sandbox::Net>>,
    /// How to reach an SSH session's server again (for the agent, uploads and downloads).
    ssh: Option<SshTarget>,
}

/// An SSH session's server. The terminal's connection is shared (OpenSSH `ControlMaster`), so
/// the agent's commands and file transfers reuse it without asking for a password again.
#[derive(Debug, Clone)]
pub struct SshTarget {
    /// `user@host` or `host`.
    pub target: String,
    pub port: Option<u16>,
    pub identity: Option<String>,
    /// The shared connection's socket (not on Windows, whose OpenSSH can't share connections).
    pub control: Option<std::path::PathBuf>,
}

impl SshTarget {
    /// Options shared by ssh and scp (`port_flag`: `-p` for ssh, `-P` for scp).
    pub fn options(&self, port_flag: &str) -> Vec<String> {
        let mut v = Vec::new();
        if let Some(p) = self.port.filter(|p| *p != 22) {
            v.extend([port_flag.to_string(), p.to_string()]);
        }
        if let Some(k) = &self.identity {
            v.extend(["-i".to_string(), k.clone()]);
        }
        if let Some(c) = &self.control {
            v.extend(["-o".to_string(), format!("ControlPath={}", c.join("%C").display())]);
        }
        v
    }
}

/// Where shared SSH connections keep their sockets: ~/.ssh/rad-cm (private). Socket paths are
/// limited to ~104 bytes and ssh adds 58 (`/` + a 40-character hash + a temporary suffix), so
/// a very long home folder means no sharing.
fn control_dir() -> Option<std::path::PathBuf> {
    if cfg!(windows) {
        return None;
    }
    let d = home()?.join(".ssh").join("rad-cm");
    if d.as_os_str().len() > 44 {
        return None;
    }
    std::fs::create_dir_all(&d).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700)).ok()?;
    }
    Some(d)
}

#[derive(Default)]
pub struct Terminals {
    sessions: Mutex<HashMap<u32, Arc<Session>>>,
    next: AtomicU32,
}

impl Terminals {
    /// What the agent works on in a session: its kind and, for SSH, the server.
    pub fn agent_target(&self, id: u32) -> CmdResult<(String, Option<SshTarget>)> {
        let s = self.get(id)?;
        let i = s.info.lock().unwrap();
        if i.exited.is_some() {
            return Err(CmdError::new("terminal", "This terminal has closed — open it again"));
        }
        Ok((i.kind.clone(), s.ssh.clone()))
    }

    /// Shows output in a session's terminal (and its scrollback) without sending it to the shell.
    pub fn show(&self, id: u32, bytes: &[u8]) {
        let Ok(s) = self.get(id) else { return };
        {
            let mut h = s.history.lock().unwrap();
            h.extend(bytes);
            let extra = h.len().saturating_sub(HISTORY);
            h.drain(..extra);
        }
        let out = s.out.lock().unwrap();
        if let Some(ch) = out.as_ref() {
            let _ = ch.send(InvokeResponseBody::Raw(bytes.to_vec()));
        }
    }

    fn get(&self, id: u32) -> CmdResult<Arc<Session>> {
        self.sessions.lock().unwrap().get(&id).cloned().ok_or_else(|| CmdError::new("not_found", "That terminal is closed"))
    }

    /// The live session of a sandbox, if one is open (a drive must never be booted twice).
    pub fn sandbox_session(&self, sandbox: &str) -> Option<u32> {
        self.sessions.lock().unwrap().values().find_map(|s| {
            let i = s.info.lock().unwrap();
            (i.sandbox.as_deref() == Some(sandbox) && i.exited.is_none()).then_some(i.id)
        })
    }

    pub fn close_all(&self) {
        for (_, s) in self.sessions.lock().unwrap().drain() {
            let _ = s.child.lock().unwrap().kill();
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct OpenArgs {
    /// "local", "ssh", "sandbox" or "rymsh".
    kind: String,
    host: Option<String>,
    user: Option<String>,
    port: Option<u16>,
    identity_file: Option<String>,
    /// "sandbox": the sandbox to start, and optionally code to run in it.
    sandbox: Option<String>,
    script: Option<ScriptArg>,
    cols: u16,
    rows: u16,
}

#[derive(Debug, Deserialize)]
pub struct ScriptArg {
    code: String,
    lang: String,
}

fn host_ok(h: &str) -> bool {
    !h.is_empty() && h.len() <= 253 && !h.starts_with('-') && h.chars().all(|c| c.is_ascii_alphanumeric() || ".-_:[]".contains(c))
}

fn user_ok(u: &str) -> bool {
    !u.is_empty() && u.len() <= 64 && !u.starts_with('-') && u.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

/// A netcat that speaks SOCKS5 (`-X 5 -x`): macOS's own, or OpenBSD netcat on Linux.
fn socks_netcat() -> Option<&'static str> {
    ["/usr/bin/nc", "/bin/nc.openbsd", "/usr/bin/nc.openbsd"].into_iter().find(|p| {
        std::path::Path::new(p).is_file() && (cfg!(target_os = "macos") || p.ends_with(".openbsd"))
    })
}

fn home() -> Option<std::path::PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(Into::into)
}

fn command(a: &OpenArgs, control: Option<&std::path::Path>) -> CmdResult<(CommandBuilder, String, Option<SshTarget>)> {
    match a.kind.as_str() {
        "ssh" => {
            let host = a.host.as_deref().map(str::trim).filter(|h| host_ok(h)).ok_or_else(|| CmdError::new("bad_request", "Enter a valid host name"))?;
            let user = a.user.as_deref().map(str::trim).filter(|u| !u.is_empty());
            if user.is_some_and(|u| !user_ok(u)) {
                return Err(CmdError::new("bad_request", "That user name isn't valid"));
            }
            let identity = a.identity_file.as_deref().map(str::trim).filter(|k| !k.is_empty()).map(|k| match (k.strip_prefix("~/"), home()) {
                (Some(rest), Some(h)) => h.join(rest).display().to_string(),
                _ => k.to_string(),
            });
            let target = match user {
                Some(u) => format!("{u}@{host}"),
                None => host.to_string(),
            };
            let t = SshTarget { target: target.clone(), port: a.port, identity, control: control.map(Into::into) };
            let mut cmd = CommandBuilder::new("ssh");
            cmd.args(t.options("-p"));
            // RAD's VPN: ssh reaches the server through the VPN's SOCKS5 proxy (never directly).
            if let Some(socks) = rad_tunnel::route::socks_for(rad_tunnel::route::Scope::Ssh).filter(|_| !rad_tunnel::route::is_local(host)) {
                let nc = socks_netcat().ok_or_else(|| {
                    CmdError::new("vpn", "SSH through the VPN needs OpenBSD netcat (on Linux: apt install netcat-openbsd). Or turn SSH off in Settings → VPN.")
                })?;
                cmd.args(["-o", &format!("ProxyCommand={nc} -X 5 -x {socks} %h %p")]);
            }
            if t.control.is_some() {
                // This terminal's connection is the one the agent and file transfers reuse.
                cmd.args(["-o", "ControlMaster=auto", "-o", "ControlPersist=60"]);
            }
            // `--` ends options: nothing after it can be read as an ssh flag.
            cmd.args(["--", &target]);
            Ok((cmd, target, Some(t)))
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
            Ok((cmd, title, None))
        }
    }
}

#[tauri::command]
pub async fn term_open(app: AppHandle, t: State<'_, Terminals>, args: OpenArgs) -> CmdResult<TermInfo> {
    let mut net = None;
    let mut sandbox = None;
    let mut ssh = None;
    let (mut cmd, title) = if args.kind == "sandbox" {
        let id = args.sandbox.clone().ok_or_else(|| CmdError::new("bad_request", "Choose a sandbox"))?;
        if let Some(open) = t.sandbox_session(&id) {
            // Already running: run the code in its shell, and show that tab.
            if let Some(sc) = &args.script {
                let runner = crate::sandbox::write_script(&crate::sandbox::Env::from_app(&app), &id, &sc.code, &sc.lang, false)?;
                term_write(t.clone(), open, format!("sh {runner}\r"))?;
            }
            let s = t.get(open)?;
            let info = s.info.lock().unwrap().clone();
            return Ok(info);
        }
        let env = crate::sandbox::Env::from_app(&app);
        let runner = match &args.script {
            Some(sc) => Some(crate::sandbox::write_script(&env, &id, &sc.code, &sc.lang, true)?),
            None => None,
        };
        let l = crate::sandbox::launch(&env, &id, runner)?;
        net = l.net;
        sandbox = Some(id);
        (l.cmd, l.title)
    } else if args.kind == "rymsh" {
        // The Rymnet shell, from the Rymnet programs RAD carries (see rymnet.rs).
        crate::rymnet::ensure_identity(&app)?;
        let mut cmd = CommandBuilder::new(crate::rymnet::program(&app, "rymsh")?);
        if let Some(s) = crate::rymnet::vpn_socks() {
            cmd.env("RYMNET_SOCKS", s);
        }
        (cmd, "rymsh".to_string())
    } else {
        let control = if args.kind == "ssh" { control_dir() } else { None };
        let (cmd, title, target) = command(&args, control.as_deref())?;
        ssh = target;
        (cmd, title)
    };
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
    let info = TermInfo { id, title, kind: args.kind.clone(), exited: None, sandbox };
    let s = Arc::new(Session {
        info: Mutex::new(info.clone()),
        writer: Mutex::new(writer),
        master: Mutex::new(pty.master),
        child: Mutex::new(child),
        history: Mutex::new(VecDeque::new()),
        out: Mutex::new(None),
        net: Mutex::new(net),
        ssh,
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
        drop(s.net.lock().unwrap().take());
        if let Some(ch) = s.out.lock().unwrap().as_ref() {
            let _ = ch.send(InvokeResponseBody::Json(json!({ "exit": msg }).to_string()));
        }
        let _ = app.emit_to("main", "term-changed", ());
    });
    Ok(info)
}

/// Output kept for the agent (the end of it, where errors and results are).
const AGENT_OUTPUT: usize = 12_000;

/// Runs a command in a sandbox session's shell, visibly, and returns its output and exit code.
/// The shell sources a small script, so `cd` and variables carry over like typed commands.
#[tauri::command]
pub async fn sandbox_exec(app: AppHandle, t: State<'_, Terminals>, id: u32, command: String, timeout_secs: Option<u64>) -> CmdResult<serde_json::Value> {
    let s = t.get(id)?;
    let (sandbox, exited) = {
        let i = s.info.lock().unwrap();
        (i.sandbox.clone(), i.exited.is_some())
    };
    let sandbox = sandbox.ok_or_else(|| CmdError::new("bad_request", "That terminal isn't a sandbox"))?;
    if exited {
        return Err(CmdError::new("terminal", "The sandbox has stopped — open it again"));
    }
    let dir = crate::sandbox::rad_dir(&crate::sandbox::Env::from_app(&app), &sandbox)?;
    let n = crate::now_ms();
    let g = format!("/root/shared/.rad/a-{n}");
    let (cmd_f, out_f, rc_f) = (dir.join(format!("a-{n}.cmd")), dir.join(format!("a-{n}.out")), dir.join(format!("a-{n}.rc")));
    std::fs::write(&cmd_f, format!("{}\n", command.trim_end())).map_err(|e| CmdError::new("io", e.to_string()))?;
    let script = format!(
        "printf '\\033[35m'; sed 's/^/agent$ /' {g}.cmd; printf '\\033[0m'\n{{ . {g}.cmd\n}} </dev/null >{g}.out 2>&1\necho $? >{g}.rc.tmp; mv {g}.rc.tmp {g}.rc\ncat {g}.out\n"
    );
    std::fs::write(dir.join(format!("a-{n}.sh")), script).map_err(|e| CmdError::new("io", e.to_string()))?;
    {
        let mut w = s.writer.lock().unwrap();
        w.write_all(format!(". {g}.sh\r").as_bytes()).and_then(|_| w.flush()).map_err(|e| CmdError::new("terminal", e.to_string()))?;
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs.unwrap_or(120).clamp(5, 1800));
    let rc = loop {
        if let Ok(v) = std::fs::read_to_string(&rc_f) {
            break v.trim().parse::<i32>().ok();
        }
        if std::time::Instant::now() > deadline || s.info.lock().unwrap().exited.is_some() {
            break None;
        }
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    };
    let raw = std::fs::read(&out_f).unwrap_or_default();
    let text = String::from_utf8_lossy(&raw);
    let truncated = text.chars().count() > AGENT_OUTPUT;
    let output: String = if truncated { text.chars().skip(text.chars().count() - AGENT_OUTPUT).collect() } else { text.into_owned() };
    if rc.is_some() {
        for f in [&cmd_f, &out_f, &rc_f, &dir.join(format!("a-{n}.sh"))] {
            let _ = std::fs::remove_file(f);
        }
    }
    Ok(serde_json::json!({ "exit_code": rc, "output": output, "truncated": truncated, "timed_out": rc.is_none() }))
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
        OpenArgs { kind: "ssh".into(), host: Some(host.into()), user: Some(user.into()), port: Some(port), identity_file: Some(key.into()), sandbox: None, script: None, cols: 80, rows: 24 }
    }

    #[test]
    fn ssh_command_is_safe() {
        let (cmd, title, _) = command(&args("coursemat.au", "deploy", 2222, "/k/id"), None).unwrap();
        let argv: Vec<String> = cmd.get_argv().iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(argv, ["ssh", "-p", "2222", "-i", "/k/id", "--", "deploy@coursemat.au"]);
        assert_eq!(title, "deploy@coursemat.au");
        let (cmd, _, t) = command(&args("coursemat.au", "", 22, ""), Some(std::path::Path::new("/h/.ssh/rad-cm"))).unwrap();
        let argv: Vec<String> = cmd.get_argv().iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(argv, ["ssh", "-o", "ControlPath=/h/.ssh/rad-cm/%C", "-o", "ControlMaster=auto", "-o", "ControlPersist=60", "--", "coursemat.au"]);
        assert_eq!(t.unwrap().options("-P"), ["-o", "ControlPath=/h/.ssh/rad-cm/%C"]);
        assert!(command(&args("-oProxyCommand=evil", "", 22, ""), None).is_err());
        assert!(command(&args("host.example", "$(id)", 22, ""), None).is_err());
        assert!(command(&args("a.b;rm -rf ~", "", 22, ""), None).is_err());
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
