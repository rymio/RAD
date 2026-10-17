//! Containers: ready-made Linux machines for Rust and Python that start, stop and keep their
//! files like Docker containers — each one a libkrun microVM, like the sandboxes.
//!
//! - A **template** per kind (Debian 13 + the toolchain + a starter project in /work/app) is
//!   built once by booting a fresh Debian drive with the internet and running its provision
//!   script. New containers are APFS clones of it: instant, and they share its disk blocks.
//! - A container's **drive** is a sparse image that only takes the space it uses. When it gets
//!   full it grows (the image doubles, and the guest fills it on the next start).
//! - Its **memory** is a ceiling: guest pages are only backed when first touched, so it grows
//!   as needed. On macOS what the guest frees stays charged to the VM until it stops (libkrun
//!   reports free pages with MADV_FREE, which Hypervisor.framework memory doesn't honour).
//! - Its **network** is a private gvproxy NAT with the internet (crates.io, PyPI) but never this
//!   Mac's localhost; listed ports are forwarded from 127.0.0.1 on this Mac. Turned off, the
//!   container is fully isolated.
//! - The **API** is rad-agent (sandbox/agent), the VM's main program, on vsock port 7000, which
//!   the launcher connects to a unix socket on this Mac: run programs (streamed output and exit
//!   codes), interactive shells, file reads and writes, stats and shutdown. The exchange folder
//!   (`/root/shared` inside) is the way to hand over whole files.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::client::{CmdError, CmdResult};
use crate::sandbox;

/// Bumped when the provision scripts change: new containers then come from a fresh template.
const TEMPLATE_VERSION: u32 = 1;
const TEMPLATE_GB: u32 = 8;
/// The size a new container's drive starts at, and the most it grows to (both sparse).
const START_GB: u32 = 32;
const MAX_GB: u32 = 1024;
const AGENT_PORT: u32 = 7000;
const AGENT_OUTPUT: usize = 12_000;
pub const PROJECT: &str = "/work/app";

pub struct Kind {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// Files of the starter project shown in the editor, relative to /work/app.
    pub files: &'static [&'static str],
    /// How "Run" runs the project.
    pub run: &'static str,
    provision: &'static str,
}

const PROFILE: &str = r#"# RAD containers: the toolchains on PATH for every shell.
export RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo
[ -d /opt/venv ] && export VIRTUAL_ENV=/opt/venv
case ":$PATH:" in *:/usr/local/cargo/bin:*) ;; *) export PATH="/opt/venv/bin:/usr/local/cargo/bin:$PATH" ;; esac
export PIP_DISABLE_PIP_VERSION_CHECK=1 CARGO_TERM_COLOR=never
"#;

/// Shared start of both provision scripts (run as root in the template VM, from /root/shared).
const PROVISION_HEAD: &str = r#"#!/bin/sh
set -e
export DEBIAN_FRONTEND=noninteractive HOME=/root LANG=C.UTF-8
echo "==> Installing system packages"
apt-get update -q >/dev/null
apt-get install -y -q --no-install-recommends ca-certificates curl git e2fsprogs util-linux procps iproute2 less nano vim-tiny build-essential pkg-config libssl-dev $PKGS >/dev/null
cp /root/shared/rad.sh /etc/profile.d/rad.sh
. /etc/profile.d/rad.sh
cat > /usr/local/bin/rad-boot <<'EOS'
#!/bin/sh
# Starts a RAD container: RAD attaches its current tools (rad-agent) read-only as "radbin".
mkdir -p /opt/rad
mount -t virtiofs -o ro radbin /opt/rad && exec /bin/sh /opt/rad/init.sh
echo "rad-boot: RAD's tools aren't attached" >&2
exit 1
EOS
chmod 755 /usr/local/bin/rad-boot
mkdir -p /work
"#;

const PROVISION_TAIL: &str = r#"
apt-get clean
rm -rf /var/lib/apt/lists/* /root/.cache
sync
echo "provisioned-ok"
"#;

const PYTHON_SH: &str = r#"
echo "==> Setting up Python"
python3 -m venv /opt/venv
/opt/venv/bin/pip install -q --upgrade pip wheel setuptools
mkdir -p /work/app
cat > /work/app/main.py <<'EOF'
import platform
import sys


def main():
    print(f"Hello from Python {platform.python_version()} on {platform.platform()}")
    print("Add packages to requirements.txt — Run installs them with pip.")
    print("Files you write to /root/shared show up on your Mac.")


if __name__ == "__main__":
    sys.exit(main())
EOF
cat > /work/app/requirements.txt <<'EOF'
# One package per line, e.g.:
# requests
# numpy
EOF
"#;

const RUST_SH: &str = r#"
echo "==> Installing Rust (rustup, stable)"
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y -q --profile minimal --default-toolchain stable --component rustfmt,clippy --no-modify-path >/dev/null
echo "==> Creating the starter project"
cd /work && USER=root cargo new -q app
cat > /work/app/src/main.rs <<'EOF'
fn main() {
    println!("Hello from Rust on {} ({})", std::env::consts::OS, std::env::consts::ARCH);
    println!("Add crates with `cargo add NAME` (or edit Cargo.toml) — Run builds and runs.");
    println!("Files you write to /root/shared show up on your Mac.");
}
EOF
echo "==> Building it once (warms up cargo)"
cd /work/app && cargo build -q
"#;

pub const KINDS: &[Kind] = &[
    Kind {
        id: "rust",
        name: "Rust",
        description: "Debian 13 with stable Rust (rustup, cargo, clippy, rustfmt). Crates from crates.io.",
        files: &["src/main.rs", "Cargo.toml"],
        run: "cargo run",
        provision: RUST_SH,
    },
    Kind {
        id: "python",
        name: "Python",
        description: "Debian 13 with Python 3 in a virtualenv (pip, build tools). Packages from PyPI.",
        files: &["main.py", "requirements.txt"],
        run: "if grep -qv '^\\s*\\(#\\|$\\)' requirements.txt 2>/dev/null; then pip install -q -r requirements.txt || exit $?; fi; python main.py",
        provision: PYTHON_SH,
    },
];

/// Boots a container (the radbin share, run by /usr/local/bin/rad-boot): mounts the exchange
/// folder, grows the filesystem to fill its drive, then hands over to rad-agent.
const INIT_SH: &str = r#"#!/bin/sh
export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
mkdir -p /root/shared /work
mountpoint -q /root/shared || mount -t virtiofs shared /root/shared
if [ -n "$RAD_HOSTNAME" ]; then
  hostname "$RAD_HOSTNAME"
  echo "$RAD_HOSTNAME" > /etc/hostname
  printf '127.0.0.1 localhost %s\n::1 localhost\n' "$RAD_HOSTNAME" > /etc/hosts
fi
# The filesystem stops 1 MiB short of the end of the drive (see sandbox.rs).
dev=$(( $(blockdev --getsize64 /dev/vda) / 4096 - 256 ))
fs=$(dumpe2fs -h /dev/vda 2>/dev/null | sed -n 's/^Block count: *//p')
if [ -n "$fs" ] && [ "$dev" -gt "$fs" ]; then
  echo "rad: growing the drive to $(( dev / 262144 )) GB"
  resize2fs /dev/vda "$dev" >/dev/null 2>&1 || echo "rad: couldn't grow the drive"
fi
. /etc/profile.d/rad.sh 2>/dev/null
exec /opt/rad/rad-agent
"#;

fn kind(id: &str) -> CmdResult<&'static Kind> {
    KINDS.iter().find(|k| k.id == id).ok_or_else(|| CmdError::new("bad_request", "Unknown container type"))
}

fn io(e: impl std::fmt::Display) -> CmdError {
    CmdError::new("io", e.to_string())
}

// ------------------------------------------------------------------ state & paths

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerConfig {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub cpus: u8,
    /// The most memory it may use; it only takes what it needs.
    pub mem_mb: u32,
    /// The drive's current size (sparse).
    pub disk_gb: u32,
    /// Internet access; off means fully isolated.
    pub network: bool,
    /// TCP ports forwarded from 127.0.0.1 on this Mac.
    #[serde(default)]
    pub ports: Vec<u16>,
    pub created_at: i64,
    #[serde(default)]
    pub template_version: u32,
    /// Filesystem use when last seen (used to grow the drive at start).
    #[serde(default)]
    pub disk_used: u64,
    #[serde(default)]
    pub disk_total: u64,
}

struct Running {
    /// None when adopted from an earlier run of RAD.
    child: Mutex<Option<Child>>,
    net: Mutex<Option<sandbox::Net>>,
    pids: Vec<u32>,
    sock: PathBuf,
    started_at: i64,
}

#[derive(Default)]
pub struct Containers {
    running: Arc<Mutex<HashMap<String, Arc<Running>>>>,
    starting: Mutex<HashSet<String>>,
    /// Template builds, one at a time.
    busy: tokio::sync::Mutex<()>,
    progress: Mutex<Option<Value>>,
    /// Streaming runs that can be cancelled.
    runs: Mutex<HashMap<String, tokio::sync::oneshot::Sender<()>>>,
}

/// Where containers live and the tools they need (so tests can run without the app).
pub struct CEnv {
    /// For the installer, the download cache and progress.
    pub sb: sandbox::Env,
    pub root: PathBuf,
    pub agent: Option<PathBuf>,
    pub changed: Arc<dyn Fn() + Send + Sync>,
}

impl CEnv {
    pub fn from_app(app: &AppHandle) -> CEnv {
        let data = app.path().app_data_dir().unwrap_or_else(|_| ".".into());
        let a = app.clone();
        let b = app.clone();
        CEnv {
            sb: sandbox::Env {
                root: data.join("sandboxes"),
                engine: sandbox::engine(app),
                progress: Box::new(move |v| {
                    *a.state::<Containers>().progress.lock().unwrap() = v.clone();
                    let _ = a.emit("container-progress", v.unwrap_or(Value::Null));
                }),
            },
            root: data.join("containers"),
            agent: agent_binary(app),
            changed: Arc::new(move || {
                let _ = b.emit("containers-changed", ());
            }),
        }
    }

    fn progress(&self, v: Option<Value>) {
        (self.sb.progress)(v);
    }
}

/// rad-agent (a Linux program): in the bundle's Resources, or (dev builds) in sandbox/bin.
fn agent_binary(app: &AppHandle) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(d) = app.path().resource_dir() {
        dirs.push(d);
    }
    if let Some(d) = std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from)) {
        dirs.push(d);
    }
    if cfg!(debug_assertions) {
        dirs.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sandbox/bin"));
    }
    dirs.into_iter().map(|d| d.join("rad-agent")).find(|p| p.is_file())
}

fn need_engine(env: &CEnv) -> CmdResult<(PathBuf, PathBuf, PathBuf)> {
    let (l, g) = sandbox::need_engine(&env.sb)?;
    let a = env.agent.clone().ok_or_else(|| {
        CmdError::new("sandbox_missing", "The container agent isn't built yet. Run ./scripts/build-sandbox.sh (and rebuild RAD).")
    })?;
    Ok((l, g, a))
}

fn dir(env: &CEnv, id: &str) -> CmdResult<PathBuf> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(CmdError::new("bad_request", "invalid container"));
    }
    Ok(env.root.join(id))
}

fn template_dir(env: &CEnv, kind: &str) -> PathBuf {
    env.root.join("templates").join(kind)
}

fn template_ready(env: &CEnv, kind: &str) -> bool {
    std::fs::read_to_string(template_dir(env, kind).join("version")).ok().and_then(|v| v.trim().parse::<u32>().ok()) == Some(TEMPLATE_VERSION)
}

pub fn load(env: &CEnv, id: &str) -> CmdResult<ContainerConfig> {
    let b = std::fs::read(dir(env, id)?.join("container.json")).map_err(|_| CmdError::new("not_found", "That container doesn't exist any more"))?;
    serde_json::from_slice(&b).map_err(|e| CmdError::new("container", e.to_string()))
}

fn save(env: &CEnv, c: &ContainerConfig) -> CmdResult<()> {
    let p = dir(env, &c.id)?.join("container.json");
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(c)?).map_err(io)?;
    std::fs::rename(tmp, p).map_err(io)
}

fn list(env: &CEnv) -> Vec<ContainerConfig> {
    let mut v: Vec<ContainerConfig> = std::fs::read_dir(&env.root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| std::fs::read(e.path().join("container.json")).ok())
        .filter_map(|b| serde_json::from_slice(&b).ok())
        .collect();
    v.sort_by_key(|c| c.created_at);
    v
}

/// The agent's socket. Unix socket paths are limited to ~104 bytes, so it lives in the
/// (per-user) temporary folder rather than under Application Support.
fn sock_path(id: &str) -> CmdResult<PathBuf> {
    let d = std::env::temp_dir().join("rad-c");
    std::fs::create_dir_all(&d).map_err(io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700));
    }
    let p = d.join(format!("{id}.sock"));
    if p.as_os_str().len() > 100 {
        return Err(CmdError::new("container", "The temporary folder's path is too long for the container's socket"));
    }
    Ok(p)
}

fn host_mem_mb() -> u32 {
    #[cfg(target_os = "macos")]
    {
        let out = Command::new("/usr/sbin/sysctl").args(["-n", "hw.memsize"]).output().ok();
        if let Some(b) = out.and_then(|o| String::from_utf8(o.stdout).ok()).and_then(|s| s.trim().parse::<u64>().ok()) {
            return (b / 1024 / 1024) as u32;
        }
    }
    8192
}

fn host_cpus() -> u8 {
    std::thread::available_parallelism().map(|n| n.get().min(8) as u8).unwrap_or(4)
}

// ------------------------------------------------------------------ talking to rad-agent

#[cfg(unix)]
mod wire {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;

    async fn open(sock: &Path, req: &Value) -> CmdResult<UnixStream> {
        let mut s = UnixStream::connect(sock).await.map_err(|_| CmdError::new("not_running", "The container isn't running — start it first"))?;
        let mut line = serde_json::to_vec(req)?;
        line.push(b'\n');
        s.write_all(&line).await.map_err(io)?;
        Ok(s)
    }

    /// One request, one answer line.
    pub async fn call(sock: &Path, req: Value, timeout: Duration) -> CmdResult<Value> {
        let fut = async {
            let s = open(sock, &req).await?;
            let mut r = BufReader::new(s);
            let mut line = String::new();
            r.read_line(&mut line).await.map_err(io)?;
            let v: Value = serde_json::from_str(&line).map_err(|_| CmdError::new("container", "The container's agent didn't answer"))?;
            if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
                return Err(CmdError::new("container", e.to_string()));
            }
            Ok(v)
        };
        tokio::time::timeout(timeout, fut).await.map_err(|_| CmdError::new("timeout", "The container didn't answer in time"))?
    }

    pub enum End {
        Exit(i32),
        Cancelled,
        TimedOut,
    }

    /// Runs a program, passing each piece of output to `on` ("o" stdout, "e" stderr).
    /// Cancelling or timing out hangs up, which stops the program.
    pub async fn exec(sock: &Path, req: Value, timeout: Option<Duration>, cancel: Option<tokio::sync::oneshot::Receiver<()>>, mut on: impl FnMut(&str, &str)) -> CmdResult<End> {
        let s = open(sock, &req).await?;
        let mut lines = BufReader::new(s).lines();
        let cancel = async move {
            match cancel {
                Some(c) => {
                    let _ = c.await;
                }
                None => std::future::pending::<()>().await,
            }
        };
        let sleep = async move {
            match timeout {
                Some(t) => tokio::time::sleep(t).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::pin!(cancel, sleep);
        loop {
            tokio::select! {
                _ = &mut cancel => return Ok(End::Cancelled),
                _ = &mut sleep => return Ok(End::TimedOut),
                l = lines.next_line() => {
                    let Some(l) = l.map_err(io)? else {
                        return Err(CmdError::new("container", "The container stopped while the program was running"));
                    };
                    let v: Value = serde_json::from_str(&l).unwrap_or(Value::Null);
                    if let Some(code) = v.get("exit").and_then(|c| c.as_i64()) {
                        return Ok(End::Exit(code as i32));
                    }
                    if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
                        return Err(CmdError::new("container", e.to_string()));
                    }
                    for k in ["o", "e"] {
                        if let Some(t) = v.get(k).and_then(|t| t.as_str()) {
                            on(k, t);
                        }
                    }
                }
            }
        }
    }

    /// Blocking: asks the guest to power off.
    pub fn shutdown_sync(sock: &Path) -> bool {
        use std::io::{Read, Write};
        let Ok(mut s) = std::os::unix::net::UnixStream::connect(sock) else { return false };
        let _ = s.set_read_timeout(Some(Duration::from_secs(3)));
        if s.write_all(b"{\"op\":\"shutdown\"}\n").is_err() {
            return false;
        }
        let mut b = [0u8; 64];
        matches!(s.read(&mut b), Ok(n) if n > 0)
    }

    /// An interactive shell: the connection carries raw terminal bytes after the handshake.
    pub fn pty(sock: &Path, term: &str, cols: u16, rows: u16) -> CmdResult<std::os::unix::net::UnixStream> {
        use std::io::{Read, Write};
        let mut s = std::os::unix::net::UnixStream::connect(sock).map_err(|_| CmdError::new("not_running", "The container isn't running — start it first"))?;
        let req = json!({"op": "pty", "id": term, "cols": cols, "rows": rows, "cwd": PROJECT});
        s.write_all(format!("{req}\n").as_bytes()).map_err(io)?;
        // Read the answer a byte at a time: whatever follows it is the shell's output.
        let mut line = Vec::new();
        let mut b = [0u8; 1];
        s.set_read_timeout(Some(Duration::from_secs(10))).map_err(io)?;
        while s.read(&mut b).map_err(io)? == 1 && b[0] != b'\n' {
            line.push(b[0]);
        }
        s.set_read_timeout(None).map_err(io)?;
        let v: Value = serde_json::from_slice(&line).map_err(|_| CmdError::new("container", "The container's agent didn't answer"))?;
        if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
            return Err(CmdError::new("container", e.to_string()));
        }
        Ok(s)
    }

    pub fn resize(sock: &Path, term: &str, cols: u16, rows: u16) {
        use std::io::Write;
        if let Ok(mut s) = std::os::unix::net::UnixStream::connect(sock) {
            let _ = s.write_all(format!("{}\n", json!({"op": "resize", "id": term, "cols": cols, "rows": rows})).as_bytes());
        }
    }
}

#[cfg(not(unix))]
mod wire {
    use super::*;
    fn no() -> CmdError {
        CmdError::new("sandbox_unsupported", "Containers need a Mac with Apple Silicon for now")
    }
    pub async fn call(_: &Path, _: Value, _: Duration) -> CmdResult<Value> {
        Err(no())
    }
    pub enum End {
        Exit(i32),
        Cancelled,
        TimedOut,
    }
    pub async fn exec(_: &Path, _: Value, _: Option<Duration>, _: Option<tokio::sync::oneshot::Receiver<()>>, _: impl FnMut(&str, &str)) -> CmdResult<End> {
        Err(no())
    }
    pub fn shutdown_sync(_: &Path) -> bool {
        false
    }
}

// ------------------------------------------------------------------ processes

fn alive(pid: u32) -> bool {
    Command::new("/bin/kill").args(["-0", &pid.to_string()]).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

fn kill(pid: u32) {
    let _ = Command::new("/bin/kill").args(["-9", &pid.to_string()]).stderr(Stdio::null()).status();
}

impl Containers {
    fn get(&self, id: &str) -> Option<Arc<Running>> {
        self.running.lock().unwrap().get(id).cloned()
    }

    /// A container VM left running by an earlier run of RAD (after a crash): adopt it if its
    /// agent answers, otherwise make sure it's gone.
    async fn adopt(&self, env: &CEnv, id: &str) {
        if self.get(id).is_some() {
            return;
        }
        let Ok(d) = dir(env, id) else { return };
        let pf = d.join("run.pids");
        let Ok(text) = std::fs::read_to_string(&pf) else { return };
        let pids: Vec<u32> = text.split_whitespace().filter_map(|p| p.parse().ok()).collect();
        let Ok(sock) = sock_path(id) else { return };
        if pids.first().is_some_and(|p| alive(*p)) && wire::call(&sock, json!({"op": "ping"}), Duration::from_secs(2)).await.is_ok() {
            let r = Arc::new(Running { child: Mutex::new(None), net: Mutex::new(None), pids, sock, started_at: crate::now_ms() });
            self.running.lock().unwrap().insert(id.to_string(), r.clone());
            watch(self.running.clone(), id.to_string(), r, env.changed.clone(), Some(d));
            return;
        }
        for p in pids {
            if alive(p) {
                kill(p);
            }
        }
        let _ = std::fs::remove_file(pf);
    }

    pub fn stop_all(&self) {
        let all: Vec<Arc<Running>> = self.running.lock().unwrap().values().cloned().collect();
        for r in &all {
            wire::shutdown_sync(&r.sock);
        }
        for r in all {
            for _ in 0..30 {
                if !r.pids.first().is_some_and(|p| alive(*p)) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            stop_processes(&r);
        }
    }
}

fn stop_processes(r: &Running) {
    if let Some(mut c) = r.child.lock().unwrap().take() {
        let _ = c.kill();
        let _ = c.wait();
    } else {
        for p in &r.pids {
            if alive(*p) {
                kill(*p);
            }
        }
    }
    drop(r.net.lock().unwrap().take());
    let _ = std::fs::remove_file(&r.sock);
}

/// Waits for a container's VM to end, then cleans up after it.
fn watch(map: Arc<Mutex<HashMap<String, Arc<Running>>>>, id: String, r: Arc<Running>, changed: Arc<dyn Fn() + Send + Sync>, d: Option<PathBuf>) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(250));
            let gone = match r.child.lock().unwrap().as_mut() {
                Some(c) => !matches!(c.try_wait(), Ok(None)),
                None => !r.pids.first().is_some_and(|p| alive(*p)),
            };
            if gone {
                break;
            }
        }
        stop_processes(&r);
        {
            // Only if the entry is still this VM (not a newer start).
            let mut m = map.lock().unwrap();
            if m.get(&id).is_some_and(|x| Arc::ptr_eq(x, &r)) {
                m.remove(&id);
                if let Some(d) = d {
                    let _ = std::fs::remove_file(d.join("run.pids"));
                }
            }
        }
        changed();
    });
}

fn running_sock(c: &Containers, id: &str) -> CmdResult<PathBuf> {
    c.get(id).map(|r| r.sock.clone()).ok_or_else(|| CmdError::new("not_running", "The container isn't running — start it first"))
}

// ------------------------------------------------------------------ templates

/// Runs a VM to completion, turning its "==> " lines into progress; returns its output's tail.
async fn run_logged(env: &CEnv, launcher: &Path, args: Vec<String>, timeout: Duration) -> CmdResult<String> {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let mut child = tokio::process::Command::new(launcher)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| CmdError::new("container", e.to_string()))?;
    let tail = Arc::new(Mutex::new(VecDeque::<String>::new()));
    let mut out = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut err = BufReader::new(child.stderr.take().unwrap()).lines();
    let work = async {
        let (mut o_done, mut e_done) = (false, false);
        while !(o_done && e_done) {
            let line = tokio::select! {
                l = out.next_line(), if !o_done => match l { Ok(Some(l)) => Some(l), _ => { o_done = true; None } },
                l = err.next_line(), if !e_done => match l { Ok(Some(l)) => Some(l), _ => { e_done = true; None } },
            };
            if let Some(l) = line {
                if let Some(p) = l.strip_prefix("==> ") {
                    env.progress(Some(json!({"phase": p})));
                }
                let mut t = tail.lock().unwrap();
                t.push_back(l);
                if t.len() > 40 {
                    t.pop_front();
                }
            }
        }
        child.wait().await
    };
    let st = tokio::time::timeout(timeout, work).await.map_err(|_| CmdError::new("container", "Setting up the container took too long"))?.map_err(io)?;
    let text = tail.lock().unwrap().iter().cloned().collect::<Vec<_>>().join("\n");
    if !st.success() || !text.contains("provisioned-ok") {
        let last: Vec<&str> = text.lines().rev().filter(|l| !l.trim().is_empty()).take(4).collect();
        return Err(CmdError::new("container", format!("Setting up the container failed: {}", last.into_iter().rev().collect::<Vec<_>>().join(" / "))));
    }
    Ok(text)
}

/// Builds a kind's template drive once: Debian, then its toolchain and starter project.
async fn ensure_template(env: &CEnv, k: &Kind) -> CmdResult<PathBuf> {
    let td = template_dir(env, k.id);
    let disk = td.join("disk.img");
    if template_ready(env, k.id) && disk.is_file() {
        return Ok(disk);
    }
    let (launcher, gvproxy, _) = need_engine(env)?;
    let tmp = td.with_extension("tmp");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("shared")).map_err(io)?;
    let tmp_disk = tmp.join("disk.img");
    sandbox::install_disk(&env.sb, "debian", &tmp_disk, TEMPLATE_GB, k.id).await?;

    env.progress(Some(json!({"phase": format!("Setting up {}", k.name)})));
    let script = format!("{PROVISION_HEAD}{}{PROVISION_TAIL}", k.provision).replace("$PKGS", if k.id == "python" { "python3 python3-venv python3-pip python3-dev libffi-dev" } else { "" });
    sandbox::write_exec(&tmp.join("shared/provision.sh"), &script)?;
    std::fs::write(tmp.join("shared/rad.sh"), PROFILE).map_err(io)?;
    let net = sandbox::start_net_with(&env.root.join("run"), &gvproxy, &[])?;
    let args: Vec<String> = [
        "--disk", &tmp_disk.display().to_string(),
        "--share", &format!("shared={}", tmp.join("shared").display()),
        "--net", &net.sock.display().to_string(),
        "--cpus", &host_cpus().to_string(),
        "--mem", "2048",
        "--env", "HOME=/root", "--env", "LANG=C.UTF-8", "--env", sandbox::GUEST_PATH,
        "--", "/usr/local/bin/rad-start", "/bin/sh", "/root/shared/provision.sh",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let r = run_logged(env, &launcher, args, Duration::from_secs(45 * 60)).await;
    drop(net);
    r?;
    let _ = std::fs::remove_dir_all(tmp.join("shared"));
    std::fs::write(tmp.join("version"), TEMPLATE_VERSION.to_string()).map_err(io)?;
    let _ = std::fs::remove_dir_all(&td);
    std::fs::rename(&tmp, &td).map_err(io)?;
    Ok(disk)
}

/// A fresh drive for a container: a copy-on-write clone of its template (APFS), grown to size.
fn clone_template(template: &Path, disk: &Path, gb: u32) -> CmdResult<()> {
    let _ = std::fs::remove_file(disk);
    // std::fs::copy clones on APFS (clonefile), so this is instant and takes no space.
    std::fs::copy(template, disk).map_err(io)?;
    let f = std::fs::OpenOptions::new().write(true).open(disk).map_err(io)?;
    f.set_len(gb as u64 * 1024 * 1024 * 1024).map_err(io)
}

// ------------------------------------------------------------------ lifecycle

#[derive(Deserialize)]
pub struct CreateArgs {
    pub kind: String,
    pub name: Option<String>,
}

pub async fn create(env: &CEnv, c: &Containers, args: CreateArgs) -> CmdResult<ContainerConfig> {
    let k = kind(&args.kind)?;
    need_engine(env)?;
    let template = {
        let _busy = c.busy.lock().await;
        let r = ensure_template(env, k).await;
        env.progress(None);
        r?
    };
    let n = list(env).iter().filter(|x| x.kind == k.id).count();
    let name = args.name.unwrap_or_default().trim().chars().take(40).collect::<String>();
    let cfg = ContainerConfig {
        id: format!("c{}", crate::now_ms()),
        name: if name.is_empty() { format!("{} {}", k.name, n + 1) } else { name },
        kind: k.id.into(),
        cpus: host_cpus().min(4),
        mem_mb: (host_mem_mb() / 2).clamp(1024, 8192),
        disk_gb: START_GB,
        network: true,
        ports: Vec::new(),
        created_at: crate::now_ms(),
        template_version: TEMPLATE_VERSION,
        disk_used: 0,
        disk_total: 0,
    };
    let d = dir(env, &cfg.id)?;
    std::fs::create_dir_all(d.join("shared")).map_err(io)?;
    if let Err(e) = clone_template(&template, &d.join("disk.img"), cfg.disk_gb).and_then(|_| save(env, &cfg)) {
        let _ = std::fs::remove_dir_all(&d);
        return Err(e);
    }
    (env.changed)();
    Ok(cfg)
}

/// Doubles the drive when it's more than 75% full (the guest fills it on its next start).
fn maybe_grow(env: &CEnv, cfg: &mut ContainerConfig) -> CmdResult<bool> {
    if cfg.disk_total == 0 || cfg.disk_used * 4 < cfg.disk_total * 3 || cfg.disk_gb >= MAX_GB {
        return Ok(false);
    }
    // Only if the image hasn't already been grown past what the guest has seen.
    if cfg.disk_gb as u64 * 1024 * 1024 * 1024 > cfg.disk_total + 2 * 1024 * 1024 * 1024 {
        return Ok(false);
    }
    cfg.disk_gb = (cfg.disk_gb * 2).min(MAX_GB);
    let f = std::fs::OpenOptions::new().write(true).open(dir(env, &cfg.id)?.join("disk.img")).map_err(io)?;
    f.set_len(cfg.disk_gb as u64 * 1024 * 1024 * 1024).map_err(io)?;
    save(env, cfg)?;
    Ok(true)
}

pub async fn start(env: &CEnv, c: &Containers, id: &str) -> CmdResult<()> {
    c.adopt(env, id).await;
    if c.get(id).is_some() {
        return Ok(());
    }
    if !c.starting.lock().unwrap().insert(id.to_string()) {
        return Err(CmdError::new("busy", "The container is already starting"));
    }
    let r = start_inner(env, c, id).await;
    c.starting.lock().unwrap().remove(id);
    r
}

async fn start_inner(env: &CEnv, c: &Containers, id: &str) -> CmdResult<()> {
    let (launcher, gvproxy, agent) = need_engine(env)?;
    let mut cfg = load(env, id)?;
    let d = dir(env, id)?;
    let disk = d.join("disk.img");
    if !disk.is_file() {
        return Err(CmdError::new("container", "This container's drive is missing — reset it"));
    }
    maybe_grow(env, &mut cfg)?;
    // A private copy of RAD's tools for this VM (so no guest can touch another's).
    let bin = d.join("radbin");
    let _ = std::fs::remove_dir_all(&bin);
    std::fs::create_dir_all(&bin).map_err(io)?;
    std::fs::copy(&agent, bin.join("rad-agent")).map_err(io)?;
    sandbox::write_exec(&bin.join("init.sh"), INIT_SH)?;
    std::fs::create_dir_all(d.join("shared")).map_err(io)?;

    let sock = sock_path(id)?;
    let _ = std::fs::remove_file(&sock);
    let net = if cfg.network { Some(sandbox::start_net_with(&env.root.join("run"), &gvproxy, &cfg.ports)?) } else { None };
    let log = std::fs::File::create(d.join("console.log")).map_err(io)?;
    let mut cmd = Command::new(&launcher);
    cmd.arg("--disk").arg(&disk);
    cmd.arg("--share").arg(format!("shared={}", d.join("shared").display()));
    cmd.arg("--share").arg(format!("radbin={}", bin.display()));
    cmd.arg("--vsock").arg(format!("{AGENT_PORT}={}", sock.display()));
    cmd.args(["--cpus", &cfg.cpus.to_string(), "--mem", &cfg.mem_mb.to_string()]);
    if let Some(n) = &net {
        cmd.arg("--net").arg(&n.sock);
    }
    for e in ["HOME=/root", "LANG=C.UTF-8", "TERM=xterm-256color", sandbox::GUEST_PATH, &format!("RAD_HOSTNAME={}", sandbox::hostname(&cfg.name))] {
        cmd.args(["--env", e]);
    }
    cmd.args(["--", "/usr/local/bin/rad-boot"]);
    cmd.stdin(Stdio::null()).stdout(log.try_clone().map_err(io)?).stderr(log);
    let child = cmd.spawn().map_err(|e| CmdError::new("container", format!("Couldn't start the container: {e}")))?;
    let mut pids = vec![child.id()];
    if let Some(n) = &net {
        pids.push(n.pid());
    }
    let _ = std::fs::write(d.join("run.pids"), pids.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(" "));
    let r = Arc::new(Running { child: Mutex::new(Some(child)), net: Mutex::new(net), pids, sock: sock.clone(), started_at: crate::now_ms() });

    // Wait for the agent to answer.
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        if wire::call(&sock, json!({"op": "ping"}), Duration::from_secs(2)).await.is_ok() {
            break;
        }
        let exited = r.child.lock().unwrap().as_mut().is_some_and(|ch| !matches!(ch.try_wait(), Ok(None)));
        if exited || std::time::Instant::now() > deadline {
            stop_processes(&r);
            let _ = std::fs::remove_file(d.join("run.pids"));
            let log = std::fs::read_to_string(d.join("console.log")).unwrap_or_default();
            let last: Vec<&str> = log.lines().rev().filter(|l| !l.trim().is_empty()).take(3).collect();
            let why = if last.is_empty() { "no output".to_string() } else { last.into_iter().rev().collect::<Vec<_>>().join(" / ") };
            return Err(CmdError::new("container", format!("The container didn't start ({why})")));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    c.running.lock().unwrap().insert(id.to_string(), r.clone());
    watch(c.running.clone(), id.to_string(), r, env.changed.clone(), Some(d));
    (env.changed)();
    Ok(())
}

pub async fn stop(env: &CEnv, c: &Containers, id: &str) -> CmdResult<()> {
    c.adopt(env, id).await;
    let Some(r) = c.get(id) else { return Ok(()) };
    // Remember how full the drive is, to grow it at the next start if needed.
    if let Ok(v) = wire::call(&r.sock, json!({"op": "ping"}), Duration::from_secs(2)).await {
        if let Ok(mut cfg) = load(env, id) {
            cfg.disk_used = v["disk_used"].as_u64().unwrap_or(0);
            cfg.disk_total = v["disk_total"].as_u64().unwrap_or(0);
            let _ = save(env, &cfg);
        }
    }
    let _ = wire::call(&r.sock, json!({"op": "shutdown"}), Duration::from_secs(3)).await;
    for _ in 0..100 {
        let gone = match r.child.lock().unwrap().as_mut() {
            Some(ch) => !matches!(ch.try_wait(), Ok(None)),
            None => !r.pids.first().is_some_and(|p| alive(*p)),
        };
        if gone {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    stop_processes(&r);
    c.running.lock().unwrap().remove(id);
    let _ = std::fs::remove_file(dir(env, id)?.join("run.pids"));
    (env.changed)();
    Ok(())
}

// ------------------------------------------------------------------ running programs

/// Runs a shell command in a container and collects its output (for agents and the API).
/// `cwd` carries over between calls like a shell's: the new one comes back.
pub async fn exec(c: &Containers, id: &str, command: &str, cwd: Option<&str>, timeout: Duration) -> CmdResult<Value> {
    let sock = running_sock(c, id)?;
    let cwd = cwd.filter(|d| d.starts_with('/') && !d.contains('\'')).unwrap_or(PROJECT);
    const MARK: &str = "\u{1e}RADCWD:";
    let script = format!("cd '{cwd}' 2>/dev/null || cd {PROJECT}\n{}\n__rad_rc=$?\nprintf '\\n\\036RADCWD:%s\\n' \"$PWD\"\nexit $__rad_rc", command.trim_end());
    let mut out = String::new();
    let end = wire::exec(&sock, json!({"op": "exec", "cmd": script, "cwd": cwd}), Some(timeout), None, |_, t| {
        out.push_str(t);
        // Keep memory bounded: only the end matters.
        if out.len() > AGENT_OUTPUT * 8 {
            let cut = out.len() - AGENT_OUTPUT * 4;
            let cut = (cut..out.len()).find(|i| out.is_char_boundary(*i)).unwrap_or(cut);
            out.drain(..cut);
        }
    })
    .await?;
    let mut new_cwd = None;
    if let Some(i) = out.rfind(MARK) {
        let rest = &out[i + MARK.len()..];
        new_cwd = rest.lines().next().map(|s| s.to_string());
        out.truncate(i);
        if out.ends_with('\n') {
            out.pop();
        }
    }
    let total = out.chars().count();
    let truncated = total > AGENT_OUTPUT;
    let output: String = if truncated { out.chars().skip(total - AGENT_OUTPUT).collect() } else { out };
    let (code, timed_out) = match end {
        wire::End::Exit(c) => (Some(c), false),
        _ => (None, true),
    };
    Ok(json!({"exit_code": code, "output": output, "truncated": truncated, "timed_out": timed_out, "cwd": new_cwd}))
}

// ------------------------------------------------------------------ commands

#[derive(Serialize)]
struct Info {
    #[serde(flatten)]
    config: ContainerConfig,
    kind_name: String,
    running: bool,
    starting: bool,
    started_at: Option<i64>,
    /// Space the drive takes on this Mac.
    disk_on_mac: u64,
    stats: Option<Value>,
    /// The drive was grown and the guest fills it on its next start.
    grow_pending: bool,
}

fn disk_on_mac(env: &CEnv, id: &str) -> u64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        dir(env, id).ok().and_then(|d| std::fs::metadata(d.join("disk.img")).ok()).map(|m| m.blocks() * 512).unwrap_or(0)
    }
    #[cfg(not(unix))]
    {
        let _ = (env, id);
        0
    }
}

async fn info(env: &CEnv, c: &Containers, mut cfg: ContainerConfig) -> Info {
    let r = c.get(&cfg.id);
    let mut stats = None;
    if let Some(r) = &r {
        if let Ok(v) = wire::call(&r.sock, json!({"op": "ping"}), Duration::from_millis(1500)).await {
            cfg.disk_used = v["disk_used"].as_u64().unwrap_or(0);
            cfg.disk_total = v["disk_total"].as_u64().unwrap_or(0);
            // Getting full while running: grow the image now; it's filled at the next start.
            let _ = maybe_grow(env, &mut cfg);
            stats = Some(v);
        }
    }
    let grow_pending = cfg.disk_total > 0 && cfg.disk_gb as u64 * 1024 * 1024 * 1024 > cfg.disk_total + 2 * 1024 * 1024 * 1024;
    Info {
        kind_name: kind(&cfg.kind).map(|k| k.name).unwrap_or("Linux").to_string(),
        running: r.is_some(),
        starting: c.starting.lock().unwrap().contains(&cfg.id),
        started_at: r.map(|r| r.started_at),
        disk_on_mac: disk_on_mac(env, &cfg.id),
        stats,
        grow_pending,
        config: cfg,
    }
}

#[tauri::command]
pub async fn container_status(app: AppHandle, c: State<'_, Containers>) -> CmdResult<Value> {
    let env = CEnv::from_app(&app);
    let mut items = Vec::new();
    for cfg in list(&env) {
        c.adopt(&env, &cfg.id).await;
        items.push(info(&env, &c, cfg).await);
    }
    Ok(json!({
        "supported": sandbox::SUPPORTED,
        "engine": sandbox::SUPPORTED && env.sb.engine.is_some() && env.agent.is_some(),
        "dev_build": cfg!(debug_assertions),
        "kinds": KINDS.iter().map(|k| json!({"id": k.id, "name": k.name, "description": k.description, "files": k.files, "run": k.run, "ready": template_ready(&env, k.id)})).collect::<Vec<_>>(),
        "containers": items,
        "progress": c.progress.lock().unwrap().clone(),
        "host": {"mem_mb": host_mem_mb(), "cpus": std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)},
        "project": PROJECT,
    }))
}

#[tauri::command]
pub async fn container_create(app: AppHandle, c: State<'_, Containers>, args: CreateArgs) -> CmdResult<ContainerConfig> {
    create(&CEnv::from_app(&app), &c, args).await
}

#[tauri::command]
pub async fn container_start(app: AppHandle, c: State<'_, Containers>, id: String) -> CmdResult<()> {
    start(&CEnv::from_app(&app), &c, &id).await
}

#[tauri::command]
pub async fn container_stop(app: AppHandle, c: State<'_, Containers>, id: String) -> CmdResult<()> {
    stop(&CEnv::from_app(&app), &c, &id).await
}

/// Back to a fresh copy of its template. The exchange folder is kept.
#[tauri::command]
pub async fn container_reset(app: AppHandle, c: State<'_, Containers>, id: String) -> CmdResult<()> {
    let env = CEnv::from_app(&app);
    let mut cfg = load(&env, &id)?;
    stop(&env, &c, &id).await?;
    let k = kind(&cfg.kind)?;
    let template = {
        let _busy = c.busy.lock().await;
        let r = ensure_template(&env, k).await;
        env.progress(None);
        r?
    };
    cfg.disk_gb = START_GB;
    cfg.disk_used = 0;
    cfg.disk_total = 0;
    cfg.template_version = TEMPLATE_VERSION;
    clone_template(&template, &dir(&env, &id)?.join("disk.img"), cfg.disk_gb)?;
    save(&env, &cfg)?;
    (env.changed)();
    Ok(())
}

#[tauri::command]
pub async fn container_delete(app: AppHandle, c: State<'_, Containers>, id: String) -> CmdResult<()> {
    let env = CEnv::from_app(&app);
    load(&env, &id)?;
    stop(&env, &c, &id).await?;
    std::fs::remove_dir_all(dir(&env, &id)?).map_err(io)?;
    (env.changed)();
    Ok(())
}

#[derive(Deserialize)]
pub struct UpdateArgs {
    id: String,
    name: Option<String>,
    cpus: Option<u8>,
    mem_mb: Option<u32>,
    network: Option<bool>,
    ports: Option<Vec<u16>>,
}

/// Name now; CPUs, memory, network and ports at the next start.
#[tauri::command]
pub fn container_update(app: AppHandle, args: UpdateArgs) -> CmdResult<ContainerConfig> {
    let env = CEnv::from_app(&app);
    let mut cfg = load(&env, &args.id)?;
    if let Some(n) = args.name.map(|n| n.trim().chars().take(40).collect::<String>()).filter(|n| !n.is_empty()) {
        cfg.name = n;
    }
    if let Some(v) = args.cpus {
        cfg.cpus = v.clamp(1, 8);
    }
    if let Some(v) = args.mem_mb {
        cfg.mem_mb = v.clamp(512, host_mem_mb().max(1024));
    }
    if let Some(v) = args.network {
        cfg.network = v;
    }
    if let Some(mut p) = args.ports {
        p.retain(|p| *p > 0);
        p.sort_unstable();
        p.dedup();
        p.truncate(16);
        cfg.ports = p;
    }
    save(&env, &cfg)?;
    (env.changed)();
    Ok(cfg)
}

/// Runs a command and returns its output (what an agent uses).
#[tauri::command]
pub async fn container_exec(c: State<'_, Containers>, id: String, command: String, cwd: Option<String>, timeout_secs: Option<u64>) -> CmdResult<Value> {
    exec(&c, &id, &command, cwd.as_deref(), Duration::from_secs(timeout_secs.unwrap_or(300).clamp(5, 3600))).await
}

/// Runs a command, streaming its output to `on_event` as {"o": text} / {"e": text}. Returns
/// the exit code, or null if it was cancelled with `container_cancel(run_id)`.
#[tauri::command]
pub async fn container_run(c: State<'_, Containers>, id: String, command: String, run_id: String, on_event: Channel<Value>) -> CmdResult<Option<i32>> {
    let sock = running_sock(&c, &id)?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    c.runs.lock().unwrap().insert(run_id.clone(), tx);
    let r = wire::exec(&sock, json!({"op": "exec", "cmd": command, "cwd": PROJECT}), None, Some(rx), |k, t| {
        let _ = on_event.send(json!({ k: t }));
    })
    .await;
    c.runs.lock().unwrap().remove(&run_id);
    match r? {
        wire::End::Exit(code) => Ok(Some(code)),
        _ => Ok(None),
    }
}

#[tauri::command]
pub fn container_cancel(c: State<'_, Containers>, run_id: String) {
    if let Some(tx) = c.runs.lock().unwrap().remove(&run_id) {
        let _ = tx.send(());
    }
}

fn guest_path(p: &str) -> CmdResult<String> {
    let p = if p.starts_with('/') { p.to_string() } else { format!("{PROJECT}/{p}") };
    if p.split('/').any(|s| s == "..") {
        return Err(CmdError::new("bad_request", "invalid path"));
    }
    Ok(p)
}

#[tauri::command]
pub async fn container_read(c: State<'_, Containers>, id: String, path: String) -> CmdResult<String> {
    let v = wire::call(&running_sock(&c, &id)?, json!({"op": "read", "path": guest_path(&path)?}), Duration::from_secs(15)).await?;
    Ok(v["data"].as_str().unwrap_or_default().to_string())
}

#[tauri::command]
pub async fn container_write(c: State<'_, Containers>, id: String, path: String, data: String) -> CmdResult<()> {
    wire::call(&running_sock(&c, &id)?, json!({"op": "write", "path": guest_path(&path)?, "data": data}), Duration::from_secs(15)).await.map(|_| ())
}

#[tauri::command]
pub async fn container_list(c: State<'_, Containers>, id: String, path: String) -> CmdResult<Value> {
    let v = wire::call(&running_sock(&c, &id)?, json!({"op": "list", "path": guest_path(&path)?}), Duration::from_secs(15)).await?;
    Ok(v["entries"].clone())
}

/// The exchange folder on this Mac (`/root/shared` inside).
#[tauri::command]
pub fn container_shared_dir(app: AppHandle, id: String) -> CmdResult<String> {
    let env = CEnv::from_app(&app);
    load(&env, &id)?;
    let d = dir(&env, &id)?.join("shared");
    let _ = std::fs::create_dir_all(&d);
    Ok(d.display().to_string())
}

// ------------------------------------------------------------------ terminals (used by terminal.rs)

/// An interactive shell in a running container, for a Terminal tab: (title, reader, writer,
/// resize, close).
#[cfg(unix)]
pub fn shell(app: &AppHandle, id: &str, term: &str, cols: u16, rows: u16) -> CmdResult<crate::terminal::Remote> {
    let env = CEnv::from_app(app);
    let cfg = load(&env, id)?;
    let c = app.state::<Containers>();
    let sock = running_sock(&c, id)?;
    let s = wire::pty(&sock, term, cols, rows)?;
    let (r, w, closer) = (s.try_clone().map_err(io)?, s.try_clone().map_err(io)?, s);
    let term = term.to_string();
    Ok(crate::terminal::Remote {
        title: cfg.name,
        reader: Box::new(r),
        writer: Box::new(w),
        resize: Box::new(move |cols, rows| wire::resize(&sock, &term, cols, rows)),
        close: Box::new(move || {
            let _ = closer.shutdown(std::net::Shutdown::Both);
        }),
    })
}

#[cfg(not(unix))]
pub fn shell(_: &AppHandle, _: &str, _: &str, _: u16, _: u16) -> CmdResult<crate::terminal::Remote> {
    Err(CmdError::new("sandbox_unsupported", "Containers need a Mac with Apple Silicon for now"))
}

#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
mod tests {
    use super::*;

    fn test_env(root: &Path) -> CEnv {
        let bin = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sandbox/bin");
        // The installer and download cache are shared with RAD's sandboxes (they're only caches).
        let sb_root = std::env::var_os("HOME").map(PathBuf::from).unwrap().join("Library/Application Support/com.rymtech.rad/sandboxes");
        CEnv {
            sb: sandbox::Env {
                root: sb_root,
                engine: Some((bin.join("rad-sandbox"), bin.join("gvproxy"))),
                progress: Box::new(|v| {
                    if let Some(p) = v.and_then(|v| v["phase"].as_str().map(String::from)) {
                        eprintln!("progress: {p}");
                    }
                }),
            },
            root: root.into(),
            agent: Some(bin.join("rad-agent")),
            changed: Arc::new(|| {}),
        }
    }

    async fn sh(c: &Containers, id: &str, cmd: &str) -> (i64, String) {
        let v = exec(c, id, cmd, None, Duration::from_secs(900)).await.unwrap();
        let out = v["output"].as_str().unwrap().to_string();
        println!("$ {cmd}\n{out}");
        (v["exit_code"].as_i64().unwrap_or(-1), out)
    }

    /// Needs the engine (./scripts/build-sandbox.sh) and the internet:
    /// cargo test -p rad-app --lib containers -- --ignored --nocapture --test-threads 1
    #[tokio::test]
    #[ignore]
    async fn python_container_end_to_end() {
        let root = std::env::temp_dir().join("rad-ct-py");
        let env = test_env(&root);
        let c = Containers::default();
        let cfg = create(&env, &c, CreateArgs { kind: "python".into(), name: Some("Py test".into()) }).await.unwrap();
        start(&env, &c, &cfg.id).await.unwrap();

        let (rc, out) = sh(&c, &cfg.id, "python --version && pip --version && hostname && pwd && df -h / | tail -1").await;
        assert_eq!(rc, 0);
        assert!(out.contains("Python 3.") && out.contains("/opt/venv") && out.contains("py-test") && out.contains("/work/app"), "{out}");

        // The starter project runs; packages install from PyPI; results reach the Mac.
        let sock = c.get(&cfg.id).unwrap().sock.clone();
        let w = |p: &str, d: &str| wire::call(&sock, json!({"op": "write", "path": p, "data": d}), Duration::from_secs(5));
        w("/work/app/requirements.txt", "six\n").await.unwrap();
        w("/work/app/main.py", "import six, json\nprint('six', six.__version__)\nopen('/root/shared/out.json','w').write(json.dumps({'ok': True}))\n").await.unwrap();
        let (rc, out) = sh(&c, &cfg.id, kind("python").unwrap().run).await;
        assert_eq!(rc, 0, "{out}");
        assert!(out.contains("six 1."), "{out}");
        assert_eq!(std::fs::read_to_string(dir(&env, &cfg.id).unwrap().join("shared/out.json")).unwrap(), "{\"ok\": true}");

        // The working directory carries over; exit codes come back.
        let v = exec(&c, &cfg.id, "cd /tmp && exit 7", None, Duration::from_secs(10)).await.unwrap();
        assert_eq!(v["exit_code"], 7);
        let v = exec(&c, &cfg.id, "cd /tmp", None, Duration::from_secs(10)).await.unwrap();
        assert_eq!(v["cwd"], "/tmp");
        let v = exec(&c, &cfg.id, "pwd", v["cwd"].as_str(), Duration::from_secs(10)).await.unwrap();
        assert_eq!(v["output"].as_str().unwrap().trim(), "/tmp");
        // Timeouts stop the program.
        let v = exec(&c, &cfg.id, "sleep 30", None, Duration::from_secs(1)).await.unwrap();
        assert_eq!(v["timed_out"], true);

        // An interactive shell.
        {
            use std::io::{Read, Write};
            let mut s = wire::pty(&c.get(&cfg.id).unwrap().sock, "t1", 100, 30).unwrap();
            s.write_all(b"stty size; echo pty-$((6*7))\n").unwrap();
            s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let mut got = String::new();
            let mut b = [0u8; 4096];
            while !got.contains("pty-42") {
                let n = s.read(&mut b).unwrap();
                assert!(n > 0, "{got}");
                got.push_str(&String::from_utf8_lossy(&b[..n]));
            }
            assert!(got.contains("30 100"), "{got}");
        }

        // A port forwarded to this Mac.
        let mut cfg2 = load(&env, &cfg.id).unwrap();
        cfg2.ports = vec![18765];
        save(&env, &cfg2).unwrap();
        stop(&env, &c, &cfg.id).await.unwrap();
        assert!(c.get(&cfg.id).is_none());
        start(&env, &c, &cfg.id).await.unwrap();
        // Files survive a restart.
        let (_, out) = sh(&c, &cfg.id, "cat requirements.txt; pip show six | head -2").await;
        assert!(out.contains("six") && out.contains("Version"), "{out}");
        sh(&c, &cfg.id, "echo served-from-container > /tmp/index.html; cd /tmp && nohup python -m http.server 18765 >/dev/null 2>&1 & sleep 1").await;
        let body = reqwest::Client::builder().no_proxy().build().unwrap().get("http://127.0.0.1:18765/index.html").send().await.unwrap().text().await.unwrap();
        assert!(body.contains("served-from-container"), "{body}");

        stop(&env, &c, &cfg.id).await.unwrap();
        let _ = std::fs::remove_dir_all(dir(&env, &cfg.id).unwrap());
    }

    #[tokio::test]
    #[ignore]
    async fn rust_container_end_to_end() {
        let root = std::env::temp_dir().join("rad-ct-rs");
        let env = test_env(&root);
        let c = Containers::default();
        let cfg = create(&env, &c, CreateArgs { kind: "rust".into(), name: None }).await.unwrap();
        start(&env, &c, &cfg.id).await.unwrap();
        let (rc, out) = sh(&c, &cfg.id, "cargo --version && cargo run -q").await;
        assert_eq!(rc, 0, "{out}");
        assert!(out.contains("Hello from Rust on linux (aarch64)"), "{out}");
        let main = "use rand::Rng;\nfn main() { let n: u32 = rand::thread_rng().gen_range(1..=6); println!(\"dice {}\", n); }\n";
        wire::call(&c.get(&cfg.id).unwrap().sock, json!({"op": "write", "path": "/work/app/src/main.rs", "data": main}), Duration::from_secs(5)).await.unwrap();
        let (rc, out) = sh(&c, &cfg.id, "cargo add -q rand@0.8 && cargo run -q").await;
        assert_eq!(rc, 0, "{out}");
        assert!(out.contains("dice "), "{out}");
        stop(&env, &c, &cfg.id).await.unwrap();
        let _ = std::fs::remove_dir_all(dir(&env, &cfg.id).unwrap());
    }

    fn footprint_mb(pid: u32) -> u64 {
        // The memory footprint (what Activity Monitor shows): pages the guest freed are
        // MADV_FREE'd by libkrun, which macOS reclaims — RSS keeps counting them until then.
        let o = Command::new("/usr/bin/top").args(["-l", "1", "-pid", &pid.to_string(), "-stats", "mem"]).output().unwrap();
        let t = String::from_utf8_lossy(&o.stdout);
        let v = t.lines().last().unwrap_or("").trim().trim_end_matches(['+', '-']);
        let (n, unit) = v.split_at(v.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(v.len()));
        let n: f64 = n.parse().unwrap_or(0.0);
        (match unit { "G" => n * 1024.0, "K" => n / 1024.0, "B" => n / 1048576.0, _ => n }) as u64
    }

    /// Memory is a ceiling that grows as the guest uses it (what it frees isn't given back
    /// until the VM stops — see the module comment).
    #[tokio::test]
    #[ignore]
    async fn memory_grows_as_needed() {
        let root = std::env::temp_dir().join("rad-ct-mem");
        let env = test_env(&root);
        let c = Containers::default();
        let cfg = create(&env, &c, CreateArgs { kind: "python".into(), name: None }).await.unwrap();
        start(&env, &c, &cfg.id).await.unwrap();
        let pid = c.get(&cfg.id).unwrap().pids[0];
        let before = footprint_mb(pid);
        let task = { let sock = c.get(&cfg.id).unwrap().sock.clone(); tokio::spawn(async move {
            wire::exec(&sock, json!({"op": "exec", "cmd": "python -c 'import time; b = bytearray(1500*1024*1024); b[::4096] = b\"x\" * len(b[::4096]); time.sleep(4)'"}), None, None, |_, t| print!("{t}")).await
        })};
        tokio::time::sleep(Duration::from_secs(3)).await;
        let peak = footprint_mb(pid);
        task.await.unwrap().unwrap();
        println!("host memory of the VM: idle {before} MB, while using 1.5 GB {peak} MB (ceiling {} MB)", cfg.mem_mb);
        stop(&env, &c, &cfg.id).await.unwrap();
        let _ = std::fs::remove_dir_all(dir(&env, &cfg.id).unwrap());
        assert!(before < 1024, "an idle container should take far less than its ceiling");
        assert!(peak > before + 1000, "allocation not seen");
    }
}
