//! rad-agent: RAD's agent inside a container VM.
//!
//! It is the VM's main program (the VM powers off when it exits) and listens on vsock port
//! 7000, which libkrun connects to a unix socket on the host. Each connection carries one
//! request: a JSON line from the host, answered with JSON lines:
//!
//!   {"op":"ping"}                                 → {"ok":true,"disk_total":…,"disk_used":…,"mem_total":…,"mem_used":…,"uptime":…}
//!   {"op":"exec","cmd":"…","cwd":"/work","env":{}} → {"o":"stdout text"}… {"e":"stderr text"}… {"exit":N}
//!   {"op":"pty","id":"t1","cols":80,"rows":24}     → {"ok":true}, then raw terminal bytes both ways
//!   {"op":"resize","id":"t1","cols":…,"rows":…}    → {"ok":true}
//!   {"op":"read","path":"/work/app/main.py"}       → {"ok":true,"data":"…"}
//!   {"op":"write","path":"…","data":"…"}           → {"ok":true}
//!   {"op":"list","path":"/work/app"}               → {"ok":true,"entries":[{"name":…,"dir":…,"size":…}]}
//!   {"op":"shutdown"}                              → {"ok":true}, then the VM powers off
//!
//! Errors are {"error":"…"}. Closing the connection while a program runs stops it (and
//! everything it started); programs it left running in the background after finishing stay.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};

const PORT: u32 = 7000;
const MAX_FILE: u64 = 8 * 1024 * 1024;

#[derive(Deserialize, Default)]
#[serde(default)]
struct Req {
    op: String,
    cmd: String,
    cwd: Option<String>,
    env: HashMap<String, String>,
    id: String,
    cols: u16,
    rows: u16,
    path: String,
    data: String,
}

/// Terminals by id, for resizing (their master side).
fn ptys() -> &'static Mutex<HashMap<String, RawFd>> {
    static P: OnceLock<Mutex<HashMap<String, RawFd>>> = OnceLock::new();
    P.get_or_init(Default::default)
}

fn main() {
    let fd = unsafe { libc::socket(libc::AF_VSOCK, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        die("socket");
    }
    let mut addr: libc::sockaddr_vm = unsafe { std::mem::zeroed() };
    addr.svm_family = libc::AF_VSOCK as libc::sa_family_t;
    addr.svm_port = PORT;
    addr.svm_cid = libc::VMADDR_CID_ANY;
    let len = std::mem::size_of::<libc::sockaddr_vm>() as libc::socklen_t;
    if unsafe { libc::bind(fd, &addr as *const _ as *const libc::sockaddr, len) } != 0 {
        die("bind");
    }
    if unsafe { libc::listen(fd, 64) } != 0 {
        die("listen");
    }
    eprintln!("rad-agent: listening on vsock port {PORT}");
    loop {
        let c = unsafe { libc::accept4(fd, std::ptr::null_mut(), std::ptr::null_mut(), libc::SOCK_CLOEXEC) };
        if c < 0 {
            continue;
        }
        // A vsock stream socket reads and writes like any other socket.
        let s = unsafe { UnixStream::from_raw_fd(c) };
        std::thread::spawn(move || {
            if let Err(e) = serve(s) {
                eprintln!("rad-agent: {e}");
            }
        });
    }
}

fn die(what: &str) -> ! {
    eprintln!("rad-agent: {what} failed: {}", std::io::Error::last_os_error());
    std::process::exit(1);
}

fn send(w: &mut impl Write, v: &Value) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(v)?;
    line.push(b'\n');
    w.write_all(&line)?;
    w.flush()
}

fn serve(s: UnixStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(s.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut w = s;
    let req: Req = match serde_json::from_str(&line) {
        Ok(r) => r,
        Err(e) => return send(&mut w, &json!({"error": format!("bad request: {e}")})),
    };
    let r = match req.op.as_str() {
        "ping" => send(&mut w, &stats()),
        "exec" => return exec(req, reader, w),
        "pty" => return pty(req, reader, w),
        "resize" => {
            let fd = ptys().lock().unwrap().get(&req.id).copied();
            match fd {
                Some(fd) => {
                    set_size(fd, req.cols, req.rows);
                    send(&mut w, &json!({"ok": true}))
                }
                None => send(&mut w, &json!({"error": "no such terminal"})),
            }
        }
        "read" => send(&mut w, &read_file(&req.path).unwrap_or_else(|e| json!({"error": e}))),
        "write" => send(&mut w, &write_file(&req.path, &req.data).map(|_| json!({"ok": true})).unwrap_or_else(|e| json!({"error": e}))),
        "list" => send(&mut w, &list(&req.path).unwrap_or_else(|e| json!({"error": e}))),
        "shutdown" => {
            unsafe { libc::sync() };
            let _ = send(&mut w, &json!({"ok": true}));
            std::thread::sleep(Duration::from_millis(50));
            std::process::exit(0);
        }
        _ => send(&mut w, &json!({"error": format!("unknown op {}", req.op)})),
    };
    r
}

// ------------------------------------------------------------------ status & files

fn stats() -> Value {
    let mut v = unsafe { std::mem::zeroed::<libc::statvfs>() };
    let (dt, du) = if unsafe { libc::statvfs(c"/".as_ptr(), &mut v) } == 0 {
        let f = v.f_frsize as u64;
        (v.f_blocks as u64 * f, (v.f_blocks as u64 - v.f_bfree as u64) * f)
    } else {
        (0, 0)
    };
    let mem = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let kb = |k: &str| mem.lines().find(|l| l.starts_with(k)).and_then(|l| l.split_whitespace().nth(1)).and_then(|n| n.parse::<u64>().ok()).unwrap_or(0) * 1024;
    let uptime = std::fs::read_to_string("/proc/uptime").ok().and_then(|u| u.split_whitespace().next().and_then(|n| n.parse::<f64>().ok())).unwrap_or(0.0);
    json!({"ok": true, "disk_total": dt, "disk_used": du, "mem_total": kb("MemTotal:"), "mem_used": kb("MemTotal:").saturating_sub(kb("MemAvailable:")), "uptime": uptime as u64})
}

fn check_path(p: &str) -> Result<(), String> {
    if !p.starts_with('/') {
        return Err("paths must be absolute".into());
    }
    Ok(())
}

fn read_file(p: &str) -> Result<Value, String> {
    check_path(p)?;
    let m = std::fs::metadata(p).map_err(|e| e.to_string())?;
    if m.len() > MAX_FILE {
        return Err("the file is too big to open here".into());
    }
    let b = std::fs::read(p).map_err(|e| e.to_string())?;
    let data = String::from_utf8(b).map_err(|_| "not a text file".to_string())?;
    Ok(json!({"ok": true, "data": data}))
}

fn write_file(p: &str, data: &str) -> Result<(), String> {
    check_path(p)?;
    if let Some(d) = std::path::Path::new(p).parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(p, data).map_err(|e| e.to_string())
}

fn list(p: &str) -> Result<Value, String> {
    check_path(p)?;
    let mut v: Vec<Value> = std::fs::read_dir(p)
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| {
            let m = e.metadata().ok();
            json!({"name": e.file_name().to_string_lossy(), "dir": m.as_ref().is_some_and(|m| m.is_dir()), "size": m.map(|m| m.len()).unwrap_or(0)})
        })
        .collect();
    v.sort_by(|a, b| (!a["dir"].as_bool().unwrap_or(false), a["name"].as_str()).cmp(&(!b["dir"].as_bool().unwrap_or(false), b["name"].as_str())));
    Ok(json!({"ok": true, "entries": v}))
}

// ------------------------------------------------------------------ programs

/// Text from a stream in chunks that never split a UTF-8 character.
struct Utf8 {
    carry: Vec<u8>,
}

impl Utf8 {
    fn push(&mut self, b: &[u8]) -> String {
        self.carry.extend_from_slice(b);
        let keep = match std::str::from_utf8(&self.carry) {
            Ok(_) => 0,
            Err(e) if e.error_len().is_none() => self.carry.len() - e.valid_up_to(),
            Err(_) => 0,
        };
        let rest = self.carry.split_off(self.carry.len() - keep);
        let text = String::from_utf8_lossy(&self.carry).into_owned();
        self.carry = rest;
        text
    }
}

fn exec(req: Req, mut reader: BufReader<UnixStream>, w: UnixStream) -> std::io::Result<()> {
    let w = Arc::new(Mutex::new(w));
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c").arg(&req.cmd).envs(&req.env).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let cwd = req.cwd.clone().unwrap_or_else(|| "/work".into());
    cmd.current_dir(if std::path::Path::new(&cwd).is_dir() { cwd.as_str() } else { "/" });
    // Its own process group, so stopping it stops everything it started.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return send(&mut *w.lock().unwrap(), &json!({"error": format!("couldn't start: {e}")})),
    };
    let pid = child.id() as i32;
    let done = Arc::new(AtomicBool::new(false));

    let pump = |mut r: Box<dyn Read + Send>, key: &'static str| {
        let w = w.clone();
        let done = done.clone();
        std::thread::spawn(move || {
            let mut u = Utf8 { carry: Vec::new() };
            let mut buf = [0u8; 16 * 1024];
            while let Ok(n) = r.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let t = u.push(&buf[..n]);
                let mut w = w.lock().unwrap();
                if !t.is_empty() && !done.load(Ordering::SeqCst) && send(&mut *w, &json!({ key: t })).is_err() {
                    break;
                }
            }
        })
    };
    let out = pump(Box::new(child.stdout.take().unwrap()), "o");
    let err = pump(Box::new(child.stderr.take().unwrap()), "e");

    // The host hanging up means "stop".
    {
        let done = done.clone();
        std::thread::spawn(move || {
            let mut b = [0u8; 256];
            loop {
                match reader.read(&mut b) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
            if !done.load(Ordering::SeqCst) {
                unsafe { libc::kill(-pid, libc::SIGKILL) };
            }
        });
    }

    let st = child.wait()?;
    // Programs left running in the background keep the pipes open: don't wait for them.
    let until = Instant::now() + Duration::from_millis(300);
    while (!out.is_finished() || !err.is_finished()) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(10));
    }
    let code = st.code().unwrap_or_else(|| 128 + st.signal().unwrap_or(0));
    let mut w = w.lock().unwrap();
    done.store(true, Ordering::SeqCst);
    send(&mut *w, &json!({"exit": code}))?;
    let _ = w.shutdown(std::net::Shutdown::Both);
    Ok(())
}

fn set_size(fd: RawFd, cols: u16, rows: u16) {
    let ws = libc::winsize { ws_row: rows.max(2), ws_col: cols.max(10), ws_xpixel: 0, ws_ypixel: 0 };
    unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &ws) };
}

/// An interactive login shell on a pseudo-terminal.
fn pty(req: Req, reader: BufReader<UnixStream>, mut w: UnixStream) -> std::io::Result<()> {
    let (mut m, mut sl) = (0, 0);
    let ws = libc::winsize { ws_row: req.rows.max(2), ws_col: req.cols.max(10), ws_xpixel: 0, ws_ypixel: 0 };
    if unsafe { libc::openpty(&mut m, &mut sl, std::ptr::null_mut(), std::ptr::null(), &ws) } != 0 {
        return send(&mut w, &json!({"error": "no pseudo-terminal"}));
    }
    let master = unsafe { OwnedFd::from_raw_fd(m) };
    let slave = unsafe { OwnedFd::from_raw_fd(sl) };
    unsafe { libc::fcntl(m, libc::F_SETFD, libc::FD_CLOEXEC) };
    let shell = if std::path::Path::new("/bin/bash").exists() { "/bin/bash" } else { "/bin/sh" };
    let mut cmd = Command::new(shell);
    cmd.arg("-l").env("TERM", "xterm-256color").env("COLORTERM", "truecolor").envs(&req.env);
    let cwd = req.cwd.clone().unwrap_or_else(|| "/work".into());
    cmd.current_dir(if std::path::Path::new(&cwd).is_dir() { cwd.as_str() } else { "/root" });
    cmd.stdin(Stdio::from(slave.try_clone()?)).stdout(Stdio::from(slave.try_clone()?)).stderr(Stdio::from(slave));
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            libc::ioctl(0, libc::TIOCSCTTY, 0);
            Ok(())
        });
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return send(&mut w, &json!({"error": format!("couldn't start a shell: {e}")})),
    };
    let pid = child.id() as i32;
    send(&mut w, &json!({"ok": true}))?;
    if !req.id.is_empty() {
        ptys().lock().unwrap().insert(req.id.clone(), master.as_raw_fd());
    }

    // Keystrokes (and anything already buffered after the request line) → the shell.
    let mut to_shell = std::fs::File::from(master.try_clone()?);
    std::thread::spawn(move || {
        let mut reader = reader;
        let mut b = [0u8; 8192];
        loop {
            match reader.read(&mut b) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if to_shell.write_all(&b[..n]).is_err() {
                        break;
                    }
                }
            }
        }
        // The host closed the terminal.
        unsafe { libc::kill(-pid, libc::SIGHUP) };
    });
    // The shell's output → the host.
    let mut from_shell = std::fs::File::from(master.try_clone()?);
    let pump = std::thread::spawn(move || {
        let mut b = [0u8; 16 * 1024];
        loop {
            match from_shell.read(&mut b) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if w.write_all(&b[..n]).is_err() {
                        break;
                    }
                }
            }
        }
        let _ = w.shutdown(std::net::Shutdown::Both);
    });
    let _ = child.wait();
    if !req.id.is_empty() {
        ptys().lock().unwrap().remove(&req.id);
    }
    // Reading the master fails (EIO) once the last program on the terminal has gone.
    let _ = pump.join();
    drop(master);
    Ok(())
}
