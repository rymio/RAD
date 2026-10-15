//! The terminal agent's hands on SSH servers and on this computer (sandboxes have their own,
//! `terminal::sandbox_exec`). Each command runs on its own, not typed into the user's shell:
//! over the SSH session's shared connection (no second login), or with `/bin/sh` locally. The
//! output is mirrored into the session's terminal so the user watches it happen, and the
//! working directory carries over from one command to the next.
//!
//! A sudo password, if the user gives one, stays in memory for that terminal only and reaches
//! the server on the command's stdin — never on a command line or in a file.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, State};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use zeroize::Zeroizing;

use crate::client::{CmdError, CmdResult};
use crate::terminal::{SshTarget, Terminals};

/// Output kept for the model (the end of it, where errors and results are).
const AGENT_OUTPUT: usize = 12_000;
/// Ends every command's output: `\x1e RAD:<exit code>:<working directory> \x1e`.
const MARK: &str = "\x1eRAD:";

#[derive(Default)]
struct Ctx {
    cwd: Option<String>,
    sudo: Option<Zeroizing<String>>,
    cancel: Option<Arc<tokio::sync::Notify>>,
}

#[derive(Default)]
pub struct Agents {
    ctx: Mutex<HashMap<u32, Ctx>>,
}

/// `'…'` for the shell.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The script that runs one command: in the last working directory, without pagers or
/// prompts, with sudo unlocked first when a password was given; it reports the exit code and
/// the new working directory after the output.
fn script(command: &str, cwd: Option<&str>, sudo: bool) -> String {
    let mut s = String::new();
    if let Some(d) = cwd {
        s.push_str(&format!("cd -- {} 2>/dev/null\n", sh_quote(d)));
    }
    s.push_str("export PAGER=cat SYSTEMD_PAGER=cat GIT_PAGER=cat MANPAGER=cat DEBIAN_FRONTEND=noninteractive NO_COLOR=1 TERM=dumb\n");
    if sudo {
        s.push_str("IFS= read -r __rad_pw; printf '%s\\n' \"$__rad_pw\" | sudo -S -p '' -v >/dev/null 2>&1; unset __rad_pw\n");
    }
    s.push_str(&format!(
        "{{ {}\n}} </dev/null 2>&1\n__rad_rc=$?\nprintf '\\036RAD:%s:%s\\036' \"$__rad_rc\" \"$PWD\"\nexit $__rad_rc\n",
        command.trim_end()
    ));
    s
}

/// Splits the end marker off the output: (output, exit code, working directory).
fn parse_output(raw: &str) -> (String, Option<i32>, Option<String>) {
    let Some(at) = raw.rfind(MARK) else { return (raw.to_string(), None, None) };
    let rest = raw[at + MARK.len()..].trim_end_matches('\x1e');
    let (rc, dir) = rest.split_once(':').unwrap_or((rest, ""));
    let out = raw[..at].strip_suffix('\n').unwrap_or(&raw[..at]).to_string();
    (out, rc.trim().parse().ok(), Some(dir.trim_end_matches('\x1e').to_string()).filter(|d| !d.is_empty()))
}

/// Removes colours and other terminal escapes.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\x1b' {
            match it.peek() {
                Some('[') => {
                    it.next();
                    while let Some(&n) = it.peek() {
                        it.next();
                        if ('@'..='~').contains(&n) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(n) = it.next() {
                        if n == '\x07' || (n == '\x1b' && it.peek() == Some(&'\\')) {
                            break;
                        }
                    }
                }
                _ => {
                    it.next();
                }
            }
        } else if c != '\r' {
            out.push(c);
        }
    }
    out
}

/// The PATH of the user's login shell: apps started from the Dock get a bare one, without
/// Homebrew and friends.
fn login_path() -> &'static str {
    static P: OnceLock<String> = OnceLock::new();
    P.get_or_init(|| {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        std::process::Command::new(shell)
            .args(["-l", "-c", "printf %s \"$PATH\""])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| std::env::var("PATH").unwrap_or_default())
    })
}

fn process(kind: &str, ssh: Option<&SshTarget>, script: &str) -> CmdResult<tokio::process::Command> {
    match (kind, ssh) {
        ("ssh", Some(t)) => {
            let mut c = tokio::process::Command::new("ssh");
            c.args(t.options("-p"));
            // Reuse the terminal's connection; never stop to ask anything.
            c.args(["-o", "ControlMaster=no", "-o", "BatchMode=yes", "-T", "--", &t.target]);
            let b64 = base64::engine::general_purpose::STANDARD.encode(script);
            // Base64 needs no quoting in any remote login shell (sh, bash, zsh, fish).
            c.arg(format!("sh -c 'eval \"$(echo {b64} | base64 -d)\"'"));
            Ok(c)
        }
        ("local", _) if !cfg!(windows) => {
            let mut c = tokio::process::Command::new("/bin/sh");
            c.args(["-c", script]).env("PATH", login_path());
            if let Some(h) = std::env::var_os("HOME") {
                c.current_dir(h);
            }
            Ok(c)
        }
        _ => Err(CmdError::new("bad_request", "The agent can't run commands in this terminal")),
    }
}

/// Shows text in the terminal, with line ends the terminal understands.
fn show(t: &Terminals, id: u32, text: &str) {
    t.show(id, text.replace("\r\n", "\n").replace('\n', "\r\n").as_bytes());
}

/// Runs one command for the agent and returns its output, exit code and working directory.
/// `quiet` (status checks) skips showing it in the terminal.
#[tauri::command]
pub async fn agent_exec(app: AppHandle, a: State<'_, Agents>, id: u32, command: String, timeout_secs: Option<u64>, quiet: Option<bool>) -> CmdResult<Value> {
    let terms = app.state::<Terminals>();
    let (kind, ssh) = terms.agent_target(id)?;
    let quiet = quiet.unwrap_or(false);
    let cancel = Arc::new(tokio::sync::Notify::new());
    let (cwd, sudo) = {
        let mut m = a.ctx.lock().unwrap();
        let c = m.entry(id).or_default();
        c.cancel = Some(cancel.clone());
        (c.cwd.clone(), c.sudo.clone())
    };
    let mut cmd = process(&kind, ssh.as_ref(), &script(&command, cwd.as_deref(), sudo.is_some()))?;
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| CmdError::new("terminal", format!("Couldn't run the command: {e}")))?;
    if !quiet {
        show(&terms, id, &format!("\n\x1b[35magent$ {}\x1b[0m\n", command.trim().replace('\n', "\n        ")));
    }
    if let Some(mut stdin) = child.stdin.take() {
        if let Some(pw) = &sudo {
            let _ = stdin.write_all(format!("{}\n", pw.as_str()).as_bytes()).await;
        }
        drop(stdin);
    }

    // stdout and stderr (ssh's own errors) into one buffer, mirrored to the terminal as they come.
    let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let mut readers = Vec::new();
    for r in [child.stdout.take().map(|o| Box::new(o) as Box<dyn tokio::io::AsyncRead + Unpin + Send>), child.stderr.take().map(|e| Box::new(e) as _)].into_iter().flatten() {
        let (buf, app) = (buf.clone(), app.clone());
        readers.push(tokio::spawn(async move {
            let mut r = r;
            let mut chunk = vec![0u8; 8192];
            while let Ok(n) = r.read(&mut chunk).await {
                if n == 0 {
                    break;
                }
                buf.lock().unwrap().extend_from_slice(&chunk[..n]);
                if !quiet {
                    // The end marker is never shown.
                    let text = String::from_utf8_lossy(&chunk[..n]);
                    let visible = text.split('\x1e').next().unwrap_or("");
                    show(&app.state::<Terminals>(), id, visible);
                }
            }
        }));
    }

    let limit = Duration::from_secs(timeout_secs.unwrap_or(180).clamp(5, 3600));
    let (status, timed_out, cancelled) = tokio::select! {
        s = child.wait() => (s.ok(), false, false),
        _ = tokio::time::sleep(limit) => (None, true, false),
        _ = cancel.notified() => (None, false, true),
    };
    if status.is_none() {
        let _ = child.kill().await;
    }
    for r in readers {
        let _ = tokio::time::timeout(Duration::from_secs(2), r).await;
    }
    a.ctx.lock().unwrap().entry(id).or_default().cancel = None;

    let raw = String::from_utf8_lossy(&buf.lock().unwrap()).into_owned();
    let (out, rc, dir) = parse_output(&raw);
    let rc = rc.or_else(|| status.and_then(|s| s.code()));
    if let Some(d) = &dir {
        a.ctx.lock().unwrap().entry(id).or_default().cwd = Some(d.clone());
    }
    if !quiet {
        let note = if cancelled { "stopped".to_string() } else if timed_out { "timed out — stopped".to_string() } else { format!("exit {}", rc.map_or("?".into(), |c| c.to_string())) };
        show(&terms, id, &format!("\n\x1b[2m[{note}]\x1b[0m\n"));
    }
    let text = strip_ansi(&out);
    let n = text.chars().count();
    let truncated = n > AGENT_OUTPUT;
    let output: String = if truncated { text.chars().skip(n - AGENT_OUTPUT).collect() } else { text };
    let cwd = a.ctx.lock().unwrap().get(&id).and_then(|c| c.cwd.clone());
    Ok(json!({
        "exit_code": if timed_out || cancelled { None } else { rc },
        "output": output,
        "truncated": truncated,
        "timed_out": timed_out,
        "cancelled": cancelled,
        "cwd": cwd,
    }))
}

/// Stops the command the agent is running in a terminal.
#[tauri::command]
pub fn agent_cancel(a: State<'_, Agents>, id: u32) {
    if let Some(n) = a.ctx.lock().unwrap().get(&id).and_then(|c| c.cancel.clone()) {
        n.notify_one();
    }
}

/// Sets (or, with `None`, forgets) the sudo password the agent uses in a terminal.
#[tauri::command]
pub fn agent_set_sudo(a: State<'_, Agents>, id: u32, password: Option<String>) {
    a.ctx.lock().unwrap().entry(id).or_default().sudo = password.filter(|p| !p.is_empty()).map(Zeroizing::new);
}

fn ssh_of(t: &Terminals, id: u32) -> CmdResult<SshTarget> {
    match t.agent_target(id)? {
        (_, Some(s)) => Ok(s),
        _ => Err(CmdError::new("bad_request", "That terminal isn't an SSH session")),
    }
}

async fn scp(args: Vec<String>) -> CmdResult<()> {
    let out = tokio::process::Command::new("scp")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|e| CmdError::new("terminal", format!("Couldn't run scp: {e}")))?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    Err(CmdError::new("terminal", err.trim().lines().last().unwrap_or("The copy failed").to_string()))
}

fn scp_base(t: &SshTarget) -> Vec<String> {
    let mut v = vec!["-B".to_string(), "-r".to_string(), "-p".to_string()];
    v.extend(t.options("-P"));
    v.extend(["-o".into(), "ControlMaster=no".into(), "--".into()]);
    v
}

/// Copies files and folders from this computer to the server: into `dest`, or the agent's
/// working directory, or the home folder.
#[tauri::command]
pub async fn ssh_upload(app: AppHandle, a: State<'_, Agents>, id: u32, paths: Vec<String>, dest: Option<String>) -> CmdResult<String> {
    let terms = app.state::<Terminals>();
    let t = ssh_of(&terms, id)?;
    if paths.is_empty() {
        return Ok(String::new());
    }
    let dest = dest.filter(|d| !d.trim().is_empty()).or_else(|| a.ctx.lock().unwrap().get(&id).and_then(|c| c.cwd.clone())).unwrap_or_default();
    if dest.contains('\n') {
        return Err(CmdError::new("bad_request", "That folder name isn't valid"));
    }
    let mut args = scp_base(&t);
    args.extend(paths.iter().cloned());
    args.push(format!("{}:{}", t.target, dest));
    scp(args).await?;
    let names: Vec<String> = paths.iter().map(|p| std::path::Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).collect();
    let where_ = if dest.is_empty() { "~".to_string() } else { dest.clone() };
    show(&terms, id, &format!("\n\x1b[35m[uploaded {} → {where_}]\x1b[0m\n", names.join(", ")));
    Ok(where_)
}

/// Copies a file or folder from the server into this computer's Downloads folder.
#[tauri::command]
pub async fn ssh_download(app: AppHandle, a: State<'_, Agents>, id: u32, remote: String) -> CmdResult<String> {
    let terms = app.state::<Terminals>();
    let t = ssh_of(&terms, id)?;
    let remote = remote.trim().to_string();
    if remote.is_empty() || remote.contains('\n') {
        return Err(CmdError::new("bad_request", "Enter the path of a file on the server"));
    }
    // Relative paths are relative to the agent's working directory.
    let remote = match (remote.starts_with('/') || remote.starts_with('~'), a.ctx.lock().unwrap().get(&id).and_then(|c| c.cwd.clone())) {
        (false, Some(cwd)) => format!("{}/{remote}", cwd.trim_end_matches('/')),
        _ => remote,
    };
    let downloads = app.path().download_dir().map_err(|e| CmdError::new("io", e.to_string()))?;
    let mut args = scp_base(&t);
    args.push(format!("{}:{}", t.target, remote));
    args.push(downloads.display().to_string());
    scp(args).await?;
    let name = remote.trim_end_matches('/').rsplit('/').next().unwrap_or(&remote).to_string();
    let local = downloads.join(&name);
    show(&terms, id, &format!("\n\x1b[35m[downloaded {remote} → {}]\x1b[0m\n", local.display()));
    Ok(local.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn run(sc: &str, stdin: Option<&str>) -> (String, Option<i32>, Option<String>) {
        let mut c = tokio::process::Command::new("/bin/sh");
        c.args(["-c", sc]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = c.spawn().unwrap();
        let mut i = child.stdin.take().unwrap();
        if let Some(s) = stdin {
            i.write_all(s.as_bytes()).await.unwrap();
        }
        drop(i);
        let o = child.wait_with_output().await.unwrap();
        parse_output(&String::from_utf8_lossy(&o.stdout))
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn script_reports_output_exit_code_and_directory() {
        let (out, rc, dir) = run(&script("cd /tmp\necho hi; echo err >&2; false", None, false), None).await;
        assert_eq!(out, "hi\nerr");
        assert_eq!(rc, Some(1));
        assert!(dir.unwrap().ends_with("tmp"));
        // The directory carries over; quotes and heredocs survive.
        let (out, rc, _) = run(&script("pwd; cat <<'EOF'\nit's $HOME\nEOF", Some("/"), false), None).await;
        assert_eq!(out, "/\nit's $HOME");
        assert_eq!(rc, Some(0));
        // Commands can't read stdin (where a sudo password would be).
        let (out, _, _) = run(&script("cat; echo done", None, false), Some("secret\n")).await;
        assert_eq!(out, "done");
        assert!(script("x", None, true).contains("sudo -S -p '' -v"));
    }

    /// Against a real server through a shared connection, e.g. with a throwaway sshd:
    /// RAD_TEST_SSH="127.0.0.1 2299 /path/to/key" cargo test -p rad-app --lib -- --ignored ssh_
    #[cfg(unix)]
    #[tokio::test]
    #[ignore]
    async fn ssh_runs_commands_over_the_shared_connection() {
        let spec = std::env::var("RAD_TEST_SSH").expect("RAD_TEST_SSH");
        let parts: Vec<&str> = spec.split_whitespace().collect();
        let (host, port, key) = (parts[0], parts[1], parts[2]);
        let dir = tempfile::Builder::new().prefix("rc").tempdir_in("/tmp").unwrap();
        let cp = format!("ControlPath={}/%C", dir.path().display());
        // The terminal's role: log in once (with the key) and keep the connection.
        let ok = std::process::Command::new("ssh")
            .args(["-p", port, "-i", key, "-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null", "-o", &cp, "-o", "ControlMaster=yes", "-o", "ControlPersist=20", "-N", "-f", host])
            .status()
            .unwrap();
        assert!(ok.success());
        // The agent's: no key, no host check — only the shared connection can let it in.
        let t = SshTarget { target: host.into(), port: port.parse().ok(), identity: None, control: Some(dir.path().into()) };
        let exec = |sc: String| {
            let mut c = process("ssh", Some(&t), &sc).unwrap();
            async move { c.output().await.unwrap() }
        };
        let o = exec(script("cd /tmp && echo \"it's $((6*7))\" && ls /nonexistent", None, false)).await;
        let (out, rc, dir2) = parse_output(&String::from_utf8_lossy(&o.stdout));
        assert!(out.starts_with("it's 42\n"), "{out:?} / {}", String::from_utf8_lossy(&o.stderr));
        assert!(rc.unwrap() != 0);
        assert!(dir2.as_deref().unwrap().ends_with("tmp"));
        let o = exec(script("pwd", dir2.as_deref(), false)).await;
        assert!(parse_output(&String::from_utf8_lossy(&o.stdout)).0.ends_with("tmp"));
        let _ = std::process::Command::new("ssh").args(["-p", port, "-o", &cp, "-O", "exit", host]).status();
    }

    #[test]
    fn markers_and_escapes() {
        assert_eq!(parse_output("a\nb\n\x1eRAD:3:/srv/app\x1e"), ("a\nb".into(), Some(3), Some("/srv/app".into())));
        assert_eq!(parse_output("partial"), ("partial".into(), None, None));
        assert_eq!(strip_ansi("\x1b[31mred\x1b[0m\r\nok\x1b]0;title\x07"), "red\nok");
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
    }
}
