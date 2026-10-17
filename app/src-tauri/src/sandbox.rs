//! Linux sandboxes: small Linux VMs to play with Python and scripts, isolated from this computer.
//!
//! Each sandbox is a libkrun microVM (Apple's Hypervisor.framework) booting a distro from its
//! own ext4 drive (a sparse disk image), so it has real Linux ownership, permissions and
//! case-sensitive names. It sees nothing of this computer except its exchange folder
//! (`~/shared` in the guest). Its network goes through gvproxy's userspace NAT: the internet is
//! reachable, this computer's localhost is not (libkrun's host-socket forwarding is always off).
//!
//! Drives are prepared by a tiny Alpine "installer" VM (mkfs.ext4 + tar as root), set up once.
//! The engine (`rad-sandbox`, libkrun, gvproxy) is built by `scripts/build-sandbox.sh`.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use futures::StreamExt;
use portable_pty::CommandBuilder;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::io::AsyncWriteExt;

use crate::client::{CmdError, CmdResult};
use crate::terminal::Terminals;

pub const SUPPORTED: bool = cfg!(all(target_os = "macos", target_arch = "aarch64"));

const ALPINE_URL: &str = "https://dl-cdn.alpinelinux.org/alpine/v3.24/releases/aarch64/alpine-minirootfs-3.24.2-aarch64.tar.gz";
const ALPINE_SHA: &str = "9bf70a7f18ea44094cbb5f70c58f9af129c8214745743db0e68e5502cc2ce773";
const ALPINE_FILE: &str = "alpine-3.24.2.tar.gz";
const LXC: &str = "https://images.linuxcontainers.org";
pub(crate) const GUEST_PATH: &str = "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

struct Distro {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    /// `None`: the pinned Alpine archive. `Some(key)`: the newest build of this Linux Containers image.
    lxc: Option<&'static str>,
}

const DISTROS: &[Distro] = &[
    Distro { id: "alpine", name: "Alpine Linux 3.24", description: "Tiny and fast — about 10 MB. Install Python with: apk add python3", lxc: None },
    Distro { id: "debian", name: "Debian 13", description: "Familiar apt and every package — about 450 MB. apt install python3", lxc: Some("debian;trixie;arm64;default") },
    Distro { id: "arch", name: "Arch Linux ARM", description: "Rolling release with pacman — about 900 MB. pacman -S python", lxc: Some("archlinux;current;arm64;default") },
];

/// Formats the drive and unpacks the distro into it (runs as root in the installer VM).
const INSTALL_SH: &str = r#"#!/bin/sh
# install.sh ARCHIVE HOSTNAME — format /dev/vda as ext4 and unpack a Linux root filesystem into it.
set -e
mkdir -p /mnt/dl /mnt/root
mount -t virtiofs dl /mnt/dl
# No discard: libkrun's macOS disk backend truncates the image file on a discard at its end.
# The filesystem also stops 1 MiB short of the end of the drive, so that can never cut into it.
blocks=$(( $(blockdev --getsize64 /dev/vda) / 4096 - 256 ))
mkfs.ext4 -q -F -E nodiscard -L rad -b 4096 /dev/vda "$blocks"
mount /dev/vda /mnt/root
tar -xpf "/mnt/dl/$1" -C /mnt/root --numeric-owner --xattrs --xattrs-include='*' 2>&1 | grep -v "Ignoring unknown extended header" || true
# DNS without systemd-resolved: gvproxy answers on the gateway.
rm -f /mnt/root/etc/resolv.conf
printf 'nameserver 192.168.127.1\nnameserver 1.1.1.1\n' > /mnt/root/etc/resolv.conf
echo "$2" > /mnt/root/etc/hostname
printf '127.0.0.1 localhost %s\n::1 localhost\n' "$2" > /mnt/root/etc/hosts
mkdir -p /mnt/root/usr/local/bin /mnt/root/root/shared
cat > /mnt/root/usr/local/bin/rad-start <<'EOS'
#!/bin/sh
# Started by RAD in every sandbox session: mount the exchange folder, then run the command.
mountpoint -q /root/shared 2>/dev/null || mount -t virtiofs shared /root/shared 2>/dev/null
[ -f /etc/hostname ] && hostname "$(cat /etc/hostname)" 2>/dev/null
if [ -x /usr/local/bin/rad-firstboot ] && [ ! -f /var/lib/rad-firstboot-done ]; then
  echo "Setting up this sandbox for the first time…"
  /usr/local/bin/rad-firstboot >/var/log/rad-firstboot.log 2>&1 && touch /var/lib/rad-firstboot-done
fi
cd /root
if [ $# -eq 0 ]; then
  if [ -x /bin/bash ]; then exec /bin/bash -l; else exec /bin/sh -l; fi
fi
exec "$@"
EOS
chmod 755 /mnt/root/usr/local/bin/rad-start
# Arch: pacman's download sandbox needs Landlock, which the sandbox kernel doesn't have;
# the keyring is set up on first boot (it needs the guest's own entropy).
if [ -f /mnt/root/etc/pacman.conf ]; then
  grep -q '^DisableSandbox' /mnt/root/etc/pacman.conf || sed -i 's/^\[options\]$/[options]\nDisableSandbox/' /mnt/root/etc/pacman.conf
  printf '#!/bin/sh\npacman-key --init && pacman-key --populate\n' > /mnt/root/usr/local/bin/rad-firstboot
  chmod 755 /mnt/root/usr/local/bin/rad-firstboot
fi
sync
echo "installed: $(du -sh /mnt/root | cut -f1)"
umount /mnt/root
"#;

const TOOLS_SH: &str = "#!/bin/sh\napk add -q --no-progress e2fsprogs tar xz && echo tools-ready\n";

// ------------------------------------------------------------------ state & paths

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxConfig {
    pub id: String,
    pub name: String,
    pub distro: String,
    pub cpus: u8,
    pub mem_mb: u32,
    pub disk_gb: u32,
    pub network: bool,
    pub created_at: i64,
}

#[derive(Default)]
pub struct Sandboxes {
    busy: tokio::sync::Mutex<()>,
    progress: Mutex<Option<Value>>,
}

/// Where sandboxes live, the engine, and where progress goes: the sandbox logic needs nothing
/// else (so tests can drive it without a running app).
pub struct Env {
    pub root: PathBuf,
    pub engine: Option<(PathBuf, PathBuf)>,
    pub progress: Box<dyn Fn(Option<Value>) + Send + Sync>,
}

impl Env {
    pub fn from_app(app: &AppHandle) -> Env {
        let a = app.clone();
        Env {
            root: app.path().app_data_dir().map(|d| d.join("sandboxes")).unwrap_or_else(|_| "sandboxes".into()),
            engine: engine(app),
            progress: Box::new(move |v| {
                *a.state::<Sandboxes>().progress.lock().unwrap() = v.clone();
                let _ = a.emit("sandbox-progress", v.unwrap_or(Value::Null));
            }),
        }
    }
}

fn root(env: &Env) -> PathBuf {
    env.root.clone()
}

fn sandbox_dir(env: &Env, id: &str) -> CmdResult<PathBuf> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(CmdError::new("bad_request", "invalid sandbox"));
    }
    Ok(root(env).join(id))
}

pub fn load(env: &Env, id: &str) -> CmdResult<SandboxConfig> {
    let p = sandbox_dir(env, id)?.join("sandbox.json");
    let b = std::fs::read(&p).map_err(|_| CmdError::new("not_found", "That sandbox doesn't exist any more"))?;
    serde_json::from_slice(&b).map_err(|e| CmdError::new("sandbox", e.to_string()))
}

fn save(env: &Env, c: &SandboxConfig) -> CmdResult<()> {
    let p = sandbox_dir(env, &c.id)?.join("sandbox.json");
    std::fs::write(p, serde_json::to_vec_pretty(c)?).map_err(|e| CmdError::new("io", e.to_string()))
}

fn list(env: &Env) -> Vec<SandboxConfig> {
    let mut v: Vec<SandboxConfig> = std::fs::read_dir(root(env))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| std::fs::read(e.path().join("sandbox.json")).ok())
        .filter_map(|b| serde_json::from_slice(&b).ok())
        .collect();
    v.sort_by_key(|c| c.created_at);
    v
}

/// The engine: `rad-sandbox` and `gvproxy`, bundled next to RAD or (dev builds) in sandbox/bin.
pub(crate) fn engine(app: &AppHandle) -> Option<(PathBuf, PathBuf)> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(d) = std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from)) {
        dirs.push(d);
    }
    if let Ok(d) = app.path().resource_dir() {
        dirs.push(d);
    }
    if cfg!(debug_assertions) {
        dirs.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sandbox/bin"));
    }
    dirs.into_iter().map(|d| (d.join("rad-sandbox"), d.join("gvproxy"))).find(|(l, g)| l.is_file() && g.is_file())
}

pub(crate) fn need_engine(env: &Env) -> CmdResult<(PathBuf, PathBuf)> {
    if !SUPPORTED {
        return Err(CmdError::new("sandbox_unsupported", "Linux sandboxes need a Mac with Apple Silicon for now"));
    }
    env.engine.clone().ok_or_else(|| {
        CmdError::new(
            "sandbox_missing",
            if cfg!(debug_assertions) {
                "The sandbox engine isn't built yet. Run ./scripts/build-sandbox.sh."
            } else {
                "This copy of RAD was built without the sandbox engine. Rebuild it with ./scripts/build-app.sh and reinstall."
            },
        )
    })
}

fn progress(env: &Env, v: Option<Value>) {
    (env.progress)(v);
}

// ------------------------------------------------------------------ network

static NET_SEQ: AtomicU32 = AtomicU32::new(0);

/// A running gvproxy: a private NAT network for one VM. Stopped when dropped.
pub struct Net {
    child: Child,
    pub sock: PathBuf,
    cfg: PathBuf,
}

impl Net {
    pub fn pid(&self) -> u32 {
        self.child.id()
    }
}

impl Drop for Net {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.sock);
        let _ = std::fs::remove_file(&self.cfg);
    }
}

/// Starts gvproxy and waits for its socket. The guest reaches the internet through NAT; the
/// address gvproxy normally maps to this computer's localhost is pointed at a reserved,
/// unroutable address (RFC 5737) instead.
fn start_net(env: &Env, gvproxy: &Path) -> CmdResult<Net> {
    start_net_with(&root(env).join("run"), gvproxy, &[])
}

/// The MAC address rad-sandbox gives the guest, and the address gvproxy always leases it.
pub(crate) const GUEST_IP: &str = "192.168.127.2";
const GUEST_MAC: &str = "5a:94:ef:e4:0c:ee";

/// gvproxy with its sockets in `dir`, forwarding these TCP ports on this computer's localhost
/// to the same ports in the guest.
pub(crate) fn start_net_with(dir: &Path, gvproxy: &Path, forwards: &[u16]) -> CmdResult<Net> {
    std::fs::create_dir_all(&dir).map_err(|e| CmdError::new("io", e.to_string()))?;
    let n = format!("{}-{}", std::process::id(), NET_SEQ.fetch_add(1, Ordering::Relaxed));
    let sock = dir.join(format!("{n}.sock"));
    let cfg = dir.join(format!("{n}.yaml"));
    if sock.as_os_str().len() > 100 {
        return Err(CmdError::new("sandbox", "RAD's data folder path is too long for the sandbox network socket"));
    }
    let _ = std::fs::remove_file(&sock);
    let mut yaml = format!("interfaces:\n  vfkit: unixgram://{}\nstack:\n  nat:\n    192.168.127.254: 192.0.2.1\n", sock.display());
    if !forwards.is_empty() {
        yaml.push_str(&format!("  dhcpStaticLeases:\n    {GUEST_IP}: {GUEST_MAC}\n  forwards:\n"));
        for p in forwards {
            yaml.push_str(&format!("    127.0.0.1:{p}: {GUEST_IP}:{p}\n"));
        }
    }
    std::fs::write(&cfg, yaml).map_err(|e| CmdError::new("io", e.to_string()))?;
    let child = Command::new(gvproxy)
        .arg("-config")
        .arg(&cfg)
        .args(["-ssh-port", "-1"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| CmdError::new("sandbox", format!("Couldn't start the sandbox network: {e}")))?;
    let mut net = Net { child, sock, cfg };
    for _ in 0..100 {
        if net.sock.exists() {
            return Ok(net);
        }
        if let Ok(Some(st)) = net.child.try_wait() {
            return Err(CmdError::new("sandbox", format!("The sandbox network stopped ({st})")));
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    Err(CmdError::new("sandbox", "The sandbox network didn't start"))
}

// ------------------------------------------------------------------ downloads & installer

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn file_sha(p: &Path) -> Option<String> {
    let mut f = std::fs::File::open(p).ok()?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h).ok()?;
    Some(hex(&h.finalize()))
}

pub(crate) async fn download(env: &Env, url: &str, sha: &str, dest: &Path, label: &str) -> CmdResult<()> {
    if dest.is_file() && file_sha(dest).as_deref() == Some(sha) {
        return Ok(());
    }
    let part = dest.with_extension("part");
    let resp = reqwest::Client::new().get(url).send().await.map_err(|e| CmdError::new("offline", format!("Download failed: {e}")))?;
    if !resp.status().is_success() {
        return Err(CmdError::new("sandbox", format!("Download failed: HTTP {}", resp.status())));
    }
    let total = resp.content_length().unwrap_or(0);
    let mut file = tokio::fs::File::create(&part).await.map_err(|e| CmdError::new("io", e.to_string()))?;
    let mut h = Sha256::new();
    let mut done = 0u64;
    let mut last = 0u64;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| CmdError::new("offline", format!("Download interrupted: {e}")))?;
        h.update(&chunk);
        file.write_all(&chunk).await.map_err(|e| CmdError::new("io", e.to_string()))?;
        done += chunk.len() as u64;
        if done - last > 1_000_000 {
            last = done;
            progress(env, Some(json!({"phase": label, "done": done, "total": total})));
        }
    }
    file.flush().await.map_err(|e| CmdError::new("io", e.to_string()))?;
    drop(file);
    if hex(&h.finalize()) != sha {
        let _ = std::fs::remove_file(&part);
        return Err(CmdError::new("sandbox", "The download didn't match its checksum — try again"));
    }
    std::fs::rename(&part, dest).map_err(|e| CmdError::new("io", e.to_string()))
}

/// The newest build of a Linux Containers image: (archive URL, SHA-256, cache file name).
async fn lxc_image(id: &str, key: &str) -> CmdResult<(String, String, String)> {
    let http = reqwest::Client::new();
    let text = |url: String| {
        let http = http.clone();
        async move {
            let r = http.get(&url).send().await.map_err(|e| CmdError::new("offline", format!("Can't reach the image server: {e}")))?;
            if !r.status().is_success() {
                return Err(CmdError::new("sandbox", format!("Image server: HTTP {}", r.status())));
            }
            r.text().await.map_err(|e| CmdError::new("offline", e.to_string()))
        }
    };
    let index = text(format!("{LXC}/meta/1.0/index-system")).await?;
    let path = index
        .lines()
        .find(|l| l.starts_with(&format!("{key};")))
        .and_then(|l| l.split(';').nth(5))
        .filter(|p| p.starts_with("/images/") && !p.contains(".."))
        .ok_or_else(|| CmdError::new("sandbox", "That distro isn't available from the image server right now"))?
        .to_string();
    let sums = text(format!("{LXC}{path}SHA256SUMS")).await?;
    let sha = sums
        .lines()
        .find_map(|l| l.strip_suffix("  rootfs.tar.xz"))
        .filter(|s| s.len() == 64)
        .ok_or_else(|| CmdError::new("sandbox", "The image server didn't list a checksum"))?
        .to_string();
    let stamp: String = path.trim_end_matches('/').rsplit('/').next().unwrap_or("latest").chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
    Ok((format!("{LXC}{path}rootfs.tar.xz"), sha, format!("{id}-{stamp}.tar.xz")))
}

/// Runs a short-lived VM to completion and returns its output.
async fn run_vm(launcher: &Path, args: Vec<String>, net: Option<&Net>, timeout: Duration) -> CmdResult<String> {
    let mut cmd = tokio::process::Command::new(launcher);
    if let Some(n) = net {
        cmd.arg("--net").arg(&n.sock);
    }
    cmd.args(&args).stdin(Stdio::null()).kill_on_drop(true);
    let out = tokio::time::timeout(timeout, cmd.output())
        .await
        .map_err(|_| CmdError::new("sandbox", "The sandbox installer took too long"))?
        .map_err(|e| CmdError::new("sandbox", e.to_string()))?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if !out.status.success() {
        return Err(CmdError::new("sandbox", format!("The installer failed: {}", text.lines().rev().take(4).collect::<Vec<_>>().join(" / "))));
    }
    Ok(text)
}

/// The installer VM's root: Alpine with e2fsprogs, GNU tar and xz. Prepared once.
pub(crate) async fn ensure_tools(env: &Env, launcher: &Path, gvproxy: &Path) -> CmdResult<PathBuf> {
    let r = root(env);
    let tools = r.join("tools");
    if tools.join(".ready").is_file() {
        return Ok(tools);
    }
    let cache = r.join("cache");
    std::fs::create_dir_all(&cache).map_err(|e| CmdError::new("io", e.to_string()))?;
    let archive = cache.join(ALPINE_FILE);
    download(env, ALPINE_URL, ALPINE_SHA, &archive, "Downloading the installer").await?;
    progress(env, Some(json!({"phase": "Preparing the installer"})));
    let tmp = r.join("tools.tmp");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| CmdError::new("io", e.to_string()))?;
    let st = Command::new("/usr/bin/tar").arg("xzf").arg(&archive).arg("-C").arg(&tmp).status().map_err(|e| CmdError::new("sandbox", e.to_string()))?;
    if !st.success() {
        return Err(CmdError::new("sandbox", "Couldn't unpack the installer"));
    }
    std::fs::write(tmp.join("etc/resolv.conf"), "nameserver 192.168.127.1\nnameserver 1.1.1.1\n").map_err(|e| CmdError::new("io", e.to_string()))?;
    write_exec(&tmp.join("root/tools.sh"), TOOLS_SH)?;
    let net = start_net(env, gvproxy)?;
    let out = run_vm(launcher, vec!["--root".into(), tmp.display().to_string(), "--".into(), "/root/tools.sh".into()], Some(&net), Duration::from_secs(300)).await?;
    drop(net);
    if !out.contains("tools-ready") {
        return Err(CmdError::new("sandbox", "Couldn't prepare the installer (is the internet reachable?)"));
    }
    let _ = std::fs::remove_dir_all(&tools);
    std::fs::rename(&tmp, &tools).map_err(|e| CmdError::new("io", e.to_string()))?;
    std::fs::write(tools.join(".ready"), "").map_err(|e| CmdError::new("io", e.to_string()))?;
    Ok(tools)
}

pub(crate) fn write_exec(p: &Path, content: &str) -> CmdResult<()> {
    std::fs::write(p, content).map_err(|e| CmdError::new("io", e.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755));
    }
    Ok(())
}

pub(crate) fn hostname(name: &str) -> String {
    let h: String = name.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let h = h.trim_matches('-').chars().take(30).collect::<String>();
    if h.is_empty() { "sandbox".into() } else { h }
}

/// Downloads the distro (cached) and installs it onto a fresh drive.
async fn install(env: &Env, c: &SandboxConfig) -> CmdResult<()> {
    let dir = sandbox_dir(env, &c.id)?;
    std::fs::create_dir_all(dir.join("shared")).map_err(|e| CmdError::new("io", e.to_string()))?;
    install_disk(env, &c.distro, &dir.join("disk.img"), c.disk_gb, &hostname(&c.name)).await
}

/// Downloads a distro (cached) and installs it onto a fresh sparse drive of `size_gb` at `disk`.
pub(crate) async fn install_disk(env: &Env, distro: &str, disk: &Path, size_gb: u32, host: &str) -> CmdResult<()> {
    let (launcher, gvproxy) = need_engine(env)?;
    let distro = DISTROS.iter().find(|d| d.id == distro).ok_or_else(|| CmdError::new("bad_request", "Unknown distro"))?;
    let tools = ensure_tools(env, &launcher, &gvproxy).await?;
    let cache = root(env).join("cache");
    let file = match distro.lxc {
        None => ALPINE_FILE.to_string(),
        Some(key) => {
            progress(env, Some(json!({"phase": format!("Finding the newest {}", distro.name)})));
            let (url, sha, file) = lxc_image(distro.id, key).await?;
            download(env, &url, &sha, &cache.join(&file), &format!("Downloading {}", distro.name)).await?;
            // Keep only the newest download of each distro.
            for e in std::fs::read_dir(&cache).into_iter().flatten().flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                if n.starts_with(&format!("{}-", distro.id)) && n != file {
                    let _ = std::fs::remove_file(e.path());
                }
            }
            file
        }
    };
    if distro.lxc.is_none() {
        download(env, ALPINE_URL, ALPINE_SHA, &cache.join(ALPINE_FILE), "Downloading Alpine").await?;
    }
    progress(env, Some(json!({"phase": format!("Installing {} on the drive", distro.name)})));
    {
        // A sparse file: it only takes the space the sandbox actually uses.
        let f = std::fs::File::create(disk).map_err(|e| CmdError::new("io", e.to_string()))?;
        f.set_len(size_gb as u64 * 1024 * 1024 * 1024).map_err(|e| CmdError::new("io", e.to_string()))?;
    }
    write_exec(&tools.join("root/install.sh"), INSTALL_SH)?;
    let args = vec![
        "--root".into(),
        tools.display().to_string(),
        "--data".into(),
        disk.display().to_string(),
        "--share".into(),
        format!("dl={}", cache.display()),
        "--mem".into(),
        "1024".into(),
        "--".into(),
        "/root/install.sh".into(),
        file,
        host.to_string(),
    ];
    let out = run_vm(&launcher, args, None, Duration::from_secs(900)).await?;
    if !out.contains("installed:") {
        return Err(CmdError::new("sandbox", "The installer didn't finish"));
    }
    Ok(())
}

// ------------------------------------------------------------------ sessions (used by terminal.rs)

pub struct Launch {
    pub cmd: CommandBuilder,
    pub net: Option<Net>,
    pub title: String,
}

/// The sandbox's private folder for RAD's scripts (`~/shared/.rad` inside the guest).
pub fn rad_dir(env: &Env, id: &str) -> CmdResult<PathBuf> {
    let dir = sandbox_dir(env, id)?.join("shared").join(".rad");
    std::fs::create_dir_all(&dir).map_err(|e| CmdError::new("io", e.to_string()))?;
    Ok(dir)
}

/// Writes code into the exchange folder with a runner script; returns the runner's guest path.
/// `then_shell`: the runner leaves an interactive shell behind (new sessions).
pub fn write_script(env: &Env, id: &str, code: &str, lang: &str, then_shell: bool) -> CmdResult<String> {
    let dir = sandbox_dir(env, id)?.join("shared").join(".rad");
    std::fs::create_dir_all(&dir).map_err(|e| CmdError::new("io", e.to_string()))?;
    let n = crate::now_ms();
    let lang = lang.to_lowercase();
    let (ext, interp, pkg) = match lang.as_str() {
        "python" | "py" | "python3" => ("py", "python3", "python3 (Arch: python)"),
        "javascript" | "js" | "node" => ("js", "node", "nodejs"),
        "ruby" | "rb" => ("rb", "ruby", "ruby"),
        "perl" | "pl" => ("pl", "perl", "perl"),
        "bash" => ("sh", "bash", "bash"),
        _ => ("sh", "sh", "busybox"),
    };
    // The code and its runner must have different names (shell code is also a .sh file).
    std::fs::write(dir.join(format!("code-{n}.{ext}")), code).map_err(|e| CmdError::new("io", e.to_string()))?;
    let tail = if then_shell { "if [ -x /bin/bash ]; then exec /bin/bash -l; else exec /bin/sh -l; fi\n" } else { "" };
    let runner = format!(
        "#!/bin/sh\ncd /root\nif ! command -v {interp} >/dev/null 2>&1; then\n  echo \"{interp} isn't installed in this sandbox. Install it with apk add / apt install / pacman -S: {pkg}\"\nelse\n  {interp} /root/shared/.rad/code-{n}.{ext}\n  printf '\\n\\033[2m[finished with exit code %s]\\033[0m\\n' \"$?\"\nfi\n{tail}"
    );
    write_exec(&dir.join(format!("run-{n}.sh")), &runner)?;
    Ok(format!("/root/shared/.rad/run-{n}.sh"))
}

/// The command for an interactive session in a sandbox (optionally running a script first).
pub fn launch(env: &Env, id: &str, runner: Option<String>) -> CmdResult<Launch> {
    let (launcher, gvproxy) = need_engine(env)?;
    let c = load(env, id)?;
    let dir = sandbox_dir(env, id)?;
    let disk = dir.join("disk.img");
    if !disk.is_file() {
        return Err(CmdError::new("sandbox", "This sandbox's drive is missing — reset it"));
    }
    let _ = std::fs::create_dir_all(dir.join("shared"));
    let net = if c.network { Some(start_net(env, &gvproxy)?) } else { None };
    let mut cmd = CommandBuilder::new(&launcher);
    cmd.arg("--disk");
    cmd.arg(&disk);
    cmd.arg("--share");
    cmd.arg(format!("shared={}", dir.join("shared").display()));
    cmd.args(["--cpus", &c.cpus.to_string(), "--mem", &c.mem_mb.to_string()]);
    if let Some(n) = &net {
        cmd.arg("--net");
        cmd.arg(&n.sock);
    }
    for e in ["TERM=xterm-256color", "COLORTERM=truecolor", "HOME=/root", "LANG=C.UTF-8", GUEST_PATH] {
        cmd.args(["--env", e]);
    }
    cmd.args(["--", "/usr/local/bin/rad-start"]);
    if let Some(r) = runner {
        cmd.args(["/bin/sh", &r]);
    }
    Ok(Launch { cmd, net, title: c.name })
}

// ------------------------------------------------------------------ commands

#[derive(Serialize)]
struct Info {
    #[serde(flatten)]
    config: SandboxConfig,
    distro_name: String,
    disk_used: u64,
    running: bool,
}

fn info(env: &Env, t: &Terminals, c: SandboxConfig) -> Info {
    #[cfg(unix)]
    let disk_used = {
        use std::os::unix::fs::MetadataExt;
        sandbox_dir(env, &c.id).ok().and_then(|d| std::fs::metadata(d.join("disk.img")).ok()).map(|m| m.blocks() * 512).unwrap_or(0)
    };
    #[cfg(not(unix))]
    let disk_used = 0;
    Info {
        distro_name: DISTROS.iter().find(|d| d.id == c.distro).map(|d| d.name).unwrap_or("Linux").to_string(),
        running: t.sandbox_session(&c.id).is_some(),
        disk_used,
        config: c,
    }
}

#[tauri::command]
pub fn sandbox_status(app: AppHandle, s: State<'_, Sandboxes>, t: State<'_, Terminals>) -> Value {
    let env = Env::from_app(&app);
    let sandboxes: Vec<Info> = list(&env).into_iter().map(|c| info(&env, &t, c)).collect();
    json!({
        "supported": SUPPORTED,
        "engine": SUPPORTED && env.engine.is_some(),
        "dev_build": cfg!(debug_assertions),
        "distros": DISTROS.iter().map(|d| json!({"id": d.id, "name": d.name, "description": d.description})).collect::<Vec<_>>(),
        "sandboxes": sandboxes,
        "progress": s.progress.lock().unwrap().clone(),
    })
}

#[derive(Deserialize)]
pub struct CreateArgs {
    pub name: String,
    pub distro: String,
    pub cpus: u8,
    pub mem_mb: u32,
    pub disk_gb: u32,
    pub network: bool,
}

/// Creates and installs a sandbox (shared by the command and tests).
pub async fn create(env: &Env, args: CreateArgs) -> CmdResult<SandboxConfig> {
    need_engine(env)?;
    let name = args.name.trim().chars().take(40).collect::<String>();
    let c = SandboxConfig {
        id: format!("sb{}", crate::now_ms()),
        name: if name.is_empty() { "Sandbox".into() } else { name },
        distro: args.distro,
        cpus: args.cpus.clamp(1, 8),
        mem_mb: args.mem_mb.clamp(256, 16384),
        disk_gb: args.disk_gb.clamp(2, 256),
        network: args.network,
        created_at: crate::now_ms(),
    };
    let dir = sandbox_dir(env, &c.id)?;
    std::fs::create_dir_all(&dir).map_err(|e| CmdError::new("io", e.to_string()))?;
    let r = install(env, &c).await;
    progress(env, None);
    match r {
        Ok(()) => {
            save(env, &c)?;
            Ok(c)
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&dir);
            Err(e)
        }
    }
}

#[tauri::command]
pub async fn sandbox_create(app: AppHandle, s: State<'_, Sandboxes>, args: CreateArgs) -> CmdResult<SandboxConfig> {
    let _busy = s.busy.try_lock().map_err(|_| CmdError::new("busy", "Another sandbox is being set up — wait for it to finish"))?;
    create(&Env::from_app(&app), args).await
}

/// Back to a freshly installed system. The exchange folder is kept.
pub async fn reset(env: &Env, id: &str) -> CmdResult<()> {
    let c = load(env, id)?;
    let r = install(env, &c).await;
    progress(env, None);
    r
}

#[tauri::command]
pub async fn sandbox_reset(app: AppHandle, s: State<'_, Sandboxes>, t: State<'_, Terminals>, id: String) -> CmdResult<()> {
    if t.sandbox_session(&id).is_some() {
        return Err(CmdError::new("busy", "Close this sandbox's terminal first"));
    }
    let _busy = s.busy.try_lock().map_err(|_| CmdError::new("busy", "Another sandbox is being set up — wait for it to finish"))?;
    reset(&Env::from_app(&app), &id).await
}

#[tauri::command]
pub fn sandbox_delete(app: AppHandle, t: State<'_, Terminals>, id: String) -> CmdResult<()> {
    let env = Env::from_app(&app);
    load(&env, &id)?;
    if t.sandbox_session(&id).is_some() {
        return Err(CmdError::new("busy", "Close this sandbox's terminal first"));
    }
    std::fs::remove_dir_all(sandbox_dir(&env, &id)?).map_err(|e| CmdError::new("io", e.to_string()))
}

#[derive(Deserialize)]
pub struct UpdateArgs {
    id: String,
    name: Option<String>,
    cpus: Option<u8>,
    mem_mb: Option<u32>,
    network: Option<bool>,
}

/// Name, CPUs, memory and network take effect the next time the sandbox starts.
#[tauri::command]
pub fn sandbox_update(app: AppHandle, args: UpdateArgs) -> CmdResult<SandboxConfig> {
    let env = Env::from_app(&app);
    let mut c = load(&env, &args.id)?;
    if let Some(n) = args.name.map(|n| n.trim().chars().take(40).collect::<String>()).filter(|n| !n.is_empty()) {
        c.name = n;
    }
    if let Some(v) = args.cpus {
        c.cpus = v.clamp(1, 8);
    }
    if let Some(v) = args.mem_mb {
        c.mem_mb = v.clamp(256, 16384);
    }
    if let Some(v) = args.network {
        c.network = v;
    }
    save(&env, &c)?;
    Ok(c)
}

/// Copies files into the sandbox's exchange folder (`~/shared` inside it).
#[tauri::command]
pub async fn sandbox_send(app: AppHandle, id: String, paths: Vec<String>) -> CmdResult<usize> {
    let env = Env::from_app(&app);
    load(&env, &id)?;
    let dest = sandbox_dir(&env, &id)?.join("shared");
    tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&dest).map_err(|e| CmdError::new("io", e.to_string()))?;
        let mut n = 0;
        for p in paths {
            let src = PathBuf::from(&p);
            let Some(name) = src.file_name() else { continue };
            if src.is_file() {
                std::fs::copy(&src, dest.join(name)).map_err(|e| CmdError::new("io", format!("{}: {e}", name.to_string_lossy())))?;
                n += 1;
            }
        }
        Ok(n)
    })
    .await
    .map_err(|e| CmdError::new("io", e.to_string()))?
}

/// The exchange folder on this computer (to show in Finder).
#[tauri::command]
pub fn sandbox_shared_dir(app: AppHandle, id: String) -> CmdResult<String> {
    let env = Env::from_app(&app);
    load(&env, &id)?;
    let d = sandbox_dir(&env, &id)?.join("shared");
    let _ = std::fs::create_dir_all(&d);
    Ok(d.display().to_string())
}

#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
mod tests {
    use super::*;
    use std::io::Read;

    /// Runs a launch command on a pseudo-terminal, like a Terminal tab does, and returns its output.
    fn run_on_pty(l: Launch) -> String {
        let pty = portable_pty::native_pty_system().openpty(portable_pty::PtySize { rows: 24, cols: 100, pixel_width: 0, pixel_height: 0 }).unwrap();
        let mut child = pty.slave.spawn_command(l.cmd).unwrap();
        drop(pty.slave);
        let mut r = pty.master.try_clone_reader().unwrap();
        let mut out = Vec::new();
        let mut buf = [0u8; 4096];
        while let Ok(n) = r.read(&mut buf) {
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
            if String::from_utf8_lossy(&out).contains("finished with exit code") {
                break;
            }
        }
        let _ = child.wait();
        drop(l.net);
        String::from_utf8_lossy(&out).into_owned()
    }

    /// Needs the engine (./scripts/build-sandbox.sh) and the internet:
    /// cargo test -p rad-app --lib sandbox -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn alpine_sandbox_end_to_end() {
        let bin = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sandbox/bin");
        let root = std::env::temp_dir().join(format!("rad-sb-{}", std::process::id()));
        let env = Env { root: root.clone(), engine: Some((bin.join("rad-sandbox"), bin.join("gvproxy"))), progress: Box::new(|_| {}) };
        // Something on this computer's localhost that the sandbox must not reach.
        let host = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = host.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for s in host.incoming().flatten() {
                let _ = std::io::Write::write_all(&mut &s, b"HTTP/1.0 200 OK\r\n\r\nSECRET-ON-HOST");
            }
        });

        let c = create(&env, CreateArgs { name: "Test box".into(), distro: "alpine".into(), cpus: 1, mem_mb: 512, disk_gb: 2, network: true }).await.unwrap();
        assert_eq!(list(&env).len(), 1);
        let code = format!(
            "echo \"hello from $(head -1 /etc/os-release) on $(hostname), root fs $(awk '$2==\"/\"{{print $3}}' /proc/mounts | tail -1)\"\n\
             wget -q -O- https://example.com | grep -o '<title>.*</title>'\n\
             wget -q -T 3 -O- http://192.168.127.254:{port}/ || echo host-blocked\n\
             wget -q -T 3 -O- http://127.0.0.1:{port}/ || echo localhost-blocked\n"
        );
        let runner = write_script(&env, &c.id, &code, "sh", false).unwrap();
        let out = run_on_pty(launch(&env, &c.id, Some(runner)).unwrap());
        println!("{out}");
        assert!(out.contains("hello from NAME=\"Alpine Linux\" on test-box, root fs ext4"), "{out}");
        assert!(out.contains("<title>Example Domain</title>"), "{out}");
        assert!(out.contains("host-blocked") && out.contains("localhost-blocked") && !out.contains("SECRET-ON-HOST"), "{out}");
        assert!(out.contains("finished with exit code 0"), "{out}");

        // Reset gives a fresh system; the exchange folder survives.
        std::fs::write(sandbox_dir(&env, &c.id).unwrap().join("shared/keep.txt"), "kept").unwrap();
        reset(&env, &c.id).await.unwrap();
        assert!(sandbox_dir(&env, &c.id).unwrap().join("shared/keep.txt").is_file());
        let _ = std::fs::remove_dir_all(&root);
    }
}
