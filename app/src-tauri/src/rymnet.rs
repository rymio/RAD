//! Rymnet integration: RAD drives the Rymnet programs (the `rymnet/` workspace) as separate
//! processes — the same way it runs its browser and media engines:
//!   - `rymwin --embed`: the Rymnet Desktop, shown inside RAD's RYMWIN view (see `browser.rs`);
//!   - `rymsh`: the Rymnet shell, opened as a Terminal tab (see `terminal.rs`);
//!   - `rymnetd`: the peer daemon, started by "Initiate" and stopped when RAD quits;
//!   - `rym`: the CLI, for the node identity and peers (`rym init`, `rym peer connect`…).
//! All of them share the node's data in `~/.rymnet`.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::client::{CmdError, CmdResult};

const EXE: &str = if cfg!(windows) { ".exe" } else { "" };
const LOG_LINES: usize = 200;

#[derive(Default)]
pub struct Rymnet {
    daemon: Mutex<Option<Child>>,
    daemon_log: Arc<Mutex<VecDeque<String>>>,
}

impl Rymnet {
    /// Stops the daemon RAD started.
    pub fn stop(&self) {
        if let Some(mut c) = self.daemon.lock().unwrap().take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

fn alive(slot: &Mutex<Option<Child>>) -> bool {
    let mut g = slot.lock().unwrap();
    let up = g.as_mut().is_some_and(|c| matches!(c.try_wait(), Ok(None)));
    if !up {
        *g = None;
    }
    up
}

/// The folder holding `rym`, `rymnetd` and `rymwin`: in the app bundle (macOS
/// `Contents/Helpers/rymnet`, elsewhere next to the app or in its resources) or, in development,
/// the `rymnet/` workspace's build folder. `RAD_RYMNET_DIR` overrides these.
pub(crate) fn bin_dir(app: &AppHandle) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(p) = std::env::var("RAD_RYMNET_DIR") {
        candidates.push(p.into());
    }
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from)) {
        candidates.push(dir.join("../Helpers/rymnet"));
        candidates.push(dir.join("rymnet"));
    }
    if let Ok(dir) = app.path().resource_dir() {
        candidates.push(dir.join("rymnet"));
    }
    if cfg!(debug_assertions) {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../rymnet/target");
        candidates.push(root.join("release"));
        candidates.push(root.join("debug"));
    }
    candidates.into_iter().find(|d| d.join(format!("rym{EXE}")).is_file())
}

/// A program's file name on this platform (`rymwin.exe` on Windows).
pub(crate) fn exe(name: &str) -> String {
    format!("{name}{EXE}")
}

/// The path of one Rymnet program (`rymsh`, `rymwin`…), or a "not built" error.
pub(crate) fn program(app: &AppHandle, name: &str) -> CmdResult<PathBuf> {
    bin(&need_dir(app)?, name)
}

/// Creates this computer's node identity (`rym init`) if it has none yet; the desktop and the
/// shell both need one.
pub(crate) fn ensure_identity(app: &AppHandle) -> CmdResult<()> {
    if !has_identity() {
        rym(&need_dir(app)?, &["init"])?;
    }
    Ok(())
}

fn need_dir(app: &AppHandle) -> CmdResult<PathBuf> {
    bin_dir(app).ok_or_else(|| {
        CmdError::new(
            "rymnet_missing",
            if cfg!(debug_assertions) {
                "Rymnet isn't built yet. Run ./scripts/build-rymnet.sh, then try again."
            } else {
                "This copy of RAD was built without Rymnet. Rebuild it with ./scripts/build-rymnet.sh and ./scripts/build-app.sh."
            },
        )
    })
}

fn bin(dir: &Path, name: &str) -> CmdResult<PathBuf> {
    let p = dir.join(format!("{name}{EXE}"));
    if p.is_file() {
        Ok(p)
    } else {
        Err(CmdError::new("rymnet_missing", format!("{name} is missing from {}", dir.display())))
    }
}

/// While RAD's VPN covers Rymnet, its programs reach peers through the VPN's proxy.
pub(crate) fn vpn_socks() -> Option<String> {
    rad_tunnel::route::socks_for(rad_tunnel::route::Scope::Rymnet).map(|s| s.to_string())
}

pub(crate) fn vpn_env_std(cmd: &mut Command) {
    match vpn_socks() {
        Some(s) => cmd.env("RYMNET_SOCKS", s),
        None => cmd.env_remove("RYMNET_SOCKS"),
    };
}

/// Runs `rym <args>`; Ok(stdout) on success, Err(stderr) otherwise.
fn rym(dir: &Path, args: &[&str]) -> CmdResult<String> {
    let mut cmd = Command::new(bin(dir, "rym")?);
    vpn_env_std(&mut cmd);
    let out = cmd
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| CmdError::new("rymnet", format!("Couldn't run rym: {e}")))?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if out.status.success() {
        Ok(stdout)
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(CmdError::new("rymnet", if stderr.is_empty() { stdout } else { stderr }))
    }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> CmdResult<T> + Send + 'static) -> CmdResult<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| CmdError::new("rymnet", e.to_string()))?
}

fn has_identity() -> bool {
    std::env::var_os("HOME").map(PathBuf::from).is_some_and(|h| h.join(".rymnet/identity").exists())
}

/// The daemon's peer port on this computer, from `rym node address` (e.g. `0.0.0.0:7444`).
fn local_peer_addr(dir: &Path) -> Option<SocketAddr> {
    let addr = rym(dir, &["node", "address"]).ok()?;
    let port = addr.rsplit(':').next()?.parse::<u16>().ok()?;
    Some(SocketAddr::from(([127, 0, 0, 1], port)))
}

fn listening(addr: SocketAddr) -> bool {
    TcpStream::connect_timeout(&addr, Duration::from_millis(250)).is_ok()
}

#[derive(Serialize)]
pub struct Peer {
    name: String,
    public_key: String,
    endpoint: String,
}

fn peers(dir: &Path) -> Vec<Peer> {
    // `rym peer list` prints `name  public_key  endpoint`, one peer per line.
    rym(dir, &["peer", "list"])
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            (f.len() == 3).then(|| Peer { name: f[0].into(), public_key: f[1].into(), endpoint: f[2].into() })
        })
        .collect()
}

#[derive(Serialize)]
pub struct Status {
    installed: bool,
    bin_dir: Option<String>,
    initialized: bool,
    node_id: Option<String>,
    listen_addr: Option<String>,
    /// "running" (started by RAD), "external" (another rymnetd is listening) or "stopped".
    daemon: &'static str,
    rymwin: bool,
    peers: Vec<Peer>,
    daemon_log: Vec<String>,
}

#[tauri::command]
pub async fn rymnet_status(app: AppHandle) -> CmdResult<Status> {
    let Some(dir) = bin_dir(&app) else {
        return Ok(Status {
            installed: false,
            bin_dir: None,
            initialized: has_identity(),
            node_id: None,
            listen_addr: None,
            daemon: "stopped",
            rymwin: false,
            peers: vec![],
            daemon_log: vec![],
        });
    };
    let app2 = app.clone();
    blocking(move || {
        let st = app2.state::<Rymnet>();
        let initialized = has_identity();
        let node_id = if initialized { rym(&dir, &["node", "id"]).ok() } else { None };
        let listen_addr = rym(&dir, &["node", "address"]).ok();
        let daemon = if alive(&st.daemon) {
            "running"
        } else if local_peer_addr(&dir).is_some_and(listening) {
            "external"
        } else {
            "stopped"
        };
        let daemon_log = st.daemon_log.lock().unwrap().iter().cloned().collect();
        Ok(Status {
            installed: true,
            bin_dir: Some(dir.display().to_string()),
            initialized,
            node_id,
            listen_addr,
            daemon,
            rymwin: app2.state::<crate::browser::Browsers>().rymwin_running(),
            peers: if initialized { peers(&dir) } else { vec![] },
            daemon_log,
        })
    })
    .await
}

fn pipe_log(r: impl Read + Send + 'static, log: Arc<Mutex<VecDeque<String>>>) {
    std::thread::spawn(move || {
        for line in BufReader::new(r).lines().map_while(Result::ok) {
            let mut l = log.lock().unwrap();
            if l.len() >= LOG_LINES {
                l.pop_front();
            }
            l.push_back(line);
        }
    });
}

/// Starts `rymnetd run` unless a daemon is already listening; waits until it accepts peers.
fn ensure_daemon(dir: &Path, st: &Rymnet, steps: &mut Vec<String>) -> CmdResult<()> {
    let addr = local_peer_addr(dir);
    if alive(&st.daemon) || addr.is_some_and(listening) {
        steps.push("Daemon already running".into());
        return Ok(());
    }
    st.daemon_log.lock().unwrap().clear();
    let mut cmd = Command::new(bin(dir, "rymnetd")?);
    vpn_env_std(&mut cmd);
    let mut child = cmd
        .arg("run")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| CmdError::new("rymnet", format!("Couldn't start rymnetd: {e}")))?;
    pipe_log(child.stdout.take().expect("piped stdout"), st.daemon_log.clone());
    pipe_log(child.stderr.take().expect("piped stderr"), st.daemon_log.clone());
    *st.daemon.lock().unwrap() = Some(child);
    for _ in 0..40 {
        std::thread::sleep(Duration::from_millis(150));
        if !alive(&st.daemon) {
            let tail: Vec<String> = st.daemon_log.lock().unwrap().iter().rev().take(5).rev().cloned().collect();
            return Err(CmdError::new("rymnet", format!("rymnetd stopped right away:\n{}", tail.join("\n"))));
        }
        if addr.is_some_and(listening) {
            break;
        }
    }
    steps.push(format!("Daemon started (peer port {})", addr.map(|a| a.port().to_string()).unwrap_or_else(|| "?".into())));
    Ok(())
}

/// Joins this computer to the Rymnet network: creates the node identity if there is none,
/// starts the peer daemon and, given another node's address, connects to it (which also
/// verifies and adds the peers that node shares). Returns what was done, step by step.
#[tauri::command]
pub async fn rymnet_initiate(app: AppHandle, address: Option<String>) -> CmdResult<Vec<String>> {
    let dir = need_dir(&app)?;
    let address = address.map(|a| a.trim().to_string()).filter(|a| !a.is_empty());
    if let Some(a) = &address {
        if a.contains(char::is_whitespace) {
            return Err(CmdError::new("rymnet", format!("“{a}” isn't a host or host:port")));
        }
    }
    let app2 = app.clone();
    blocking(move || {
        let st = app2.state::<Rymnet>();
        let mut steps = Vec::new();
        let fresh = !has_identity();
        rym(&dir, &["init"])?;
        steps.push(if fresh { "Node identity created" } else { "Node identity found" }.into());
        ensure_daemon(&dir, &st, &mut steps)?;
        if let Some(a) = address {
            let out = rym(&dir, &["peer", "connect", &a])?;
            steps.extend(out.lines().map(str::to_string));
        }
        Ok(steps)
    })
    .await
}

#[tauri::command]
pub async fn rymnet_remove_peer(app: AppHandle, name: String) -> CmdResult<()> {
    let dir = need_dir(&app)?;
    blocking(move || rym(&dir, &["peer", "remove", &name]).map(|_| ())).await
}

#[tauri::command]
pub fn rymnet_stop_daemon(st: State<'_, Rymnet>) {
    st.stop();
}

/// The RYMWIN User Guide, built into RAD: filed in the vault for chat (RAG) and given to the
/// RYMNET assistant.
#[tauri::command]
pub fn rymnet_guide() -> &'static str {
    include_str!("../../../rymnet/docs/RYMWIN_USER_GUIDE.md")
}

#[derive(Serialize)]
pub struct Exec {
    exit_code: Option<i32>,
    output: String,
    timed_out: bool,
}

/// Runs one `rym` command for the RYMNET assistant (never a shell): stdout and stderr together,
/// stopped after `timeout_secs`.
#[tauri::command]
pub async fn rymnet_exec(app: AppHandle, args: Vec<String>, timeout_secs: Option<u64>) -> CmdResult<Exec> {
    let dir = need_dir(&app)?;
    let home = std::env::var("HOME").unwrap_or_default();
    let args: Vec<String> = args.into_iter().map(|a| if a == "~" || a.starts_with("~/") { format!("{home}{}", &a[1..]) } else { a }).collect();
    let limit = Duration::from_secs(timeout_secs.unwrap_or(120).clamp(5, 600));
    blocking(move || {
        let mut cmd = Command::new(bin(&dir, "rym")?);
        vpn_env_std(&mut cmd);
        let mut child = cmd
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| CmdError::new("rymnet", format!("Couldn't run rym: {e}")))?;
        let log: Arc<Mutex<VecDeque<String>>> = Arc::default();
        let (out, err) = (child.stdout.take().expect("piped stdout"), child.stderr.take().expect("piped stderr"));
        let readers = [out_reader(out, log.clone()), out_reader(err, log.clone())];
        let start = std::time::Instant::now();
        let (exit_code, timed_out) = loop {
            if let Ok(Some(s)) = child.try_wait() {
                break (s.code(), false);
            }
            if start.elapsed() > limit {
                let _ = child.kill();
                let _ = child.wait();
                break (None, true);
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        for r in readers {
            let _ = r.join();
        }
        let mut output: String = log.lock().unwrap().iter().cloned().collect::<Vec<_>>().join("\n");
        if output.len() > 20_000 {
            let cut = output.len() - 20_000;
            let cut = (cut..output.len()).find(|&i| output.is_char_boundary(i)).unwrap_or(cut);
            output = format!("…(earlier output cut)\n{}", &output[cut..]);
        }
        Ok(Exec { exit_code, output, timed_out })
    })
    .await
}

fn out_reader(r: impl Read + Send + 'static, log: Arc<Mutex<VecDeque<String>>>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        for line in BufReader::new(r).lines().map_while(Result::ok) {
            log.lock().unwrap().push_back(line);
        }
    })
}

/// Pastes the system clipboard's text into the embedded RYMWIN (⌘V in its view).
#[tauri::command]
pub fn rymwin_paste(bs: State<'_, crate::browser::Browsers>) -> CmdResult<()> {
    let text = arboard::Clipboard::new().and_then(|mut c| c.get_text()).unwrap_or_default();
    if text.is_empty() {
        return Ok(());
    }
    bs.rymwin_send(&serde_json::json!({"cmd": "paste", "text": text}))
}

/// A fresh, empty folder for vault documents on their way into RYMWIN.
#[tauri::command]
pub fn rymnet_temp_dir() -> CmdResult<String> {
    let dir = std::env::temp_dir().join(format!("rad-to-rymwin-{}", crate::now_ms()));
    std::fs::create_dir_all(&dir).map_err(|e| CmdError::new("rymnet", e.to_string()))?;
    Ok(dir.display().to_string())
}
