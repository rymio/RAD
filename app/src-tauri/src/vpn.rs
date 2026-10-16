//! RAD's VPN (see `crates/rad-tunnel`): WireGuard inside RAD, no admin rights. Settings live in
//! `vpn.json` next to RAD's config; the private and preshared keys live in the OS keychain.
//! While it's on, the kinds of traffic the user chose go through it and nothing falls back to the
//! open network: if the tunnel can't reach the server, those connections fail instead.

use std::path::PathBuf;
use std::sync::Mutex;

use rad_tunnel::route::{self, Route};
use rad_tunnel::{Status, Tunnel, WgConfig};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::client::{CmdError, CmdResult};
use crate::{secrets, App};

const KEY: &str = "vpn-private-key";
const PSK: &str = "vpn-preshared-key";

/// What goes through the VPN while it's on.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Scopes {
    /// RAD server, AI providers, model and voice downloads.
    pub services: bool,
    pub browsers: bool,
    pub email: bool,
    pub ssh: bool,
    pub rymnet: bool,
}

impl Default for Scopes {
    fn default() -> Self {
        Scopes { services: true, browsers: true, email: true, ssh: true, rymnet: true }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub endpoint: String,
    pub peer_public_key: String,
    pub address: Vec<String>,
    pub dns: Vec<String>,
    pub keepalive: Option<u16>,
    pub mtu: Option<u16>,
    pub scopes: Scopes,
    /// Turn the VPN back on when RAD starts, if it was on when RAD quit.
    pub auto_connect: bool,
    /// It was on (for `auto_connect`).
    pub was_on: bool,
}

#[derive(Default)]
pub struct Vpn {
    tunnel: tokio::sync::Mutex<Option<Tunnel>>,
    settings: Mutex<Option<Settings>>,
}

#[derive(Serialize)]
pub struct Info {
    settings: Settings,
    /// This device's public key (goes into the server's [Peer]).
    public_key: Option<String>,
    has_psk: bool,
    configured: bool,
    on: bool,
    socks: Option<String>,
    status: Option<Status>,
}

fn dir(app: &AppHandle) -> PathBuf {
    app.state::<App>().cfg_dir.clone()
}

fn load(app: &AppHandle) -> Settings {
    let v = app.state::<Vpn>();
    let mut g = v.settings.lock().unwrap();
    if g.is_none() {
        *g = Some(std::fs::read(dir(app).join("vpn.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default());
    }
    g.clone().unwrap()
}

fn store(app: &AppHandle, s: &Settings) -> CmdResult<()> {
    let d = dir(app);
    std::fs::create_dir_all(&d).map_err(|e| CmdError::new("vpn", e.to_string()))?;
    std::fs::write(d.join("vpn.json"), serde_json::to_vec_pretty(s)?).map_err(|e| CmdError::new("vpn", e.to_string()))?;
    *app.state::<Vpn>().settings.lock().unwrap() = Some(s.clone());
    Ok(())
}

fn wg_config(app: &AppHandle, s: &Settings) -> CmdResult<WgConfig> {
    let d = dir(app);
    let private_key = secrets::get_named(&d, KEY).ok_or_else(|| CmdError::new("vpn_setup", "Create or import this device's key first (Settings → VPN)"))?;
    let c = WgConfig {
        private_key,
        peer_public_key: s.peer_public_key.clone(),
        preshared_key: secrets::get_named(&d, PSK),
        endpoint: s.endpoint.clone(),
        address: s.address.clone(),
        dns: s.dns.clone(),
        keepalive: s.keepalive,
        mtu: s.mtu,
    };
    c.validate().map_err(|e| CmdError::new("vpn_setup", format!("{e:#}")))?;
    Ok(c)
}

fn apply_route(s: &Settings, socks: Option<std::net::SocketAddr>) {
    route::set(socks.map(|socks| Route {
        socks,
        services: s.scopes.services,
        browsers: s.scopes.browsers,
        email: s.scopes.email,
        ssh: s.scopes.ssh,
        rymnet: s.scopes.rymnet,
        tunnel_nets: s.address.iter().filter_map(|a| rad_tunnel::config::parse_cidr(a).ok()).map(|mut c| {
            // The device's /32 sits in the server's subnet; treat the surrounding /24 (/64) as inside.
            c.prefix = c.prefix.min(if c.addr.is_ipv4() { 24 } else { 64 });
            c
        }).collect(),
    }));
}

async fn info(app: &AppHandle) -> Info {
    let s = load(app);
    let d = dir(app);
    let key = secrets::get_named(&d, KEY);
    let t = app.state::<Vpn>();
    let t = t.tunnel.lock().await;
    Info {
        configured: key.is_some() && !s.endpoint.is_empty() && !s.peer_public_key.is_empty() && !s.address.is_empty(),
        public_key: key.and_then(|k| rad_tunnel::public_key(&k).ok()),
        has_psk: secrets::get_named(&d, PSK).is_some(),
        on: t.is_some(),
        socks: t.as_ref().map(|t| t.socks_addr().to_string()),
        status: t.as_ref().map(|t| t.status()),
        settings: s,
    }
}

/// Tells the views (Chromium restarts to pick the proxy up or drop it).
fn changed(app: &AppHandle, on: bool) {
    crate::browser::vpn_changed(app);
    let _ = app.emit("vpn-changed", on);
}

async fn start(app: &AppHandle) -> CmdResult<()> {
    let s = load(app);
    let cfg = wg_config(app, &s)?;
    let vpn = app.state::<Vpn>();
    let mut g = vpn.tunnel.lock().await;
    *g = None;
    route::set(None);
    let t = Tunnel::start(&cfg).await.map_err(|e| CmdError::new("vpn", format!("{e:#}")))?;
    apply_route(&s, Some(t.socks_addr()));
    *g = Some(t);
    Ok(())
}

#[tauri::command]
pub async fn vpn_get(app: AppHandle) -> CmdResult<Info> {
    Ok(info(&app).await)
}

/// Turns the VPN on or off.
#[tauri::command]
pub async fn vpn_set(app: AppHandle, on: bool) -> CmdResult<Info> {
    if on {
        start(&app).await?;
    } else {
        route::set(None);
        *app.state::<Vpn>().tunnel.lock().await = None;
    }
    let mut s = load(&app);
    s.was_on = on;
    store(&app, &s)?;
    changed(&app, on);
    Ok(info(&app).await)
}

#[derive(Deserialize)]
pub struct SettingsIn {
    endpoint: String,
    peer_public_key: String,
    address: Vec<String>,
    dns: Vec<String>,
    keepalive: Option<u16>,
    scopes: Scopes,
    auto_connect: bool,
    /// None: keep; Some(""): remove; Some(key): set.
    preshared_key: Option<String>,
}

/// Saves the server details and choices; a running tunnel picks up new choices at once and
/// reconnects if the server details changed.
#[tauri::command]
pub async fn vpn_save(app: AppHandle, settings: SettingsIn) -> CmdResult<Info> {
    let d = dir(&app);
    if let Some(p) = &settings.preshared_key {
        if p.trim().is_empty() {
            secrets::delete_named(&d, PSK);
        } else {
            rad_tunnel::config::decode_key(p, "preshared key").map_err(|e| CmdError::new("bad_request", format!("{e:#}")))?;
            secrets::set_named(&d, PSK, p.trim()).map_err(|e| CmdError::new("vpn", e.to_string()))?;
        }
    }
    let old = load(&app);
    let clean = |v: Vec<String>| v.into_iter().map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect::<Vec<_>>();
    let s = Settings {
        endpoint: settings.endpoint.trim().to_string(),
        peer_public_key: settings.peer_public_key.trim().to_string(),
        address: clean(settings.address),
        dns: clean(settings.dns),
        keepalive: settings.keepalive.filter(|k| *k > 0),
        mtu: old.mtu,
        scopes: settings.scopes,
        auto_connect: settings.auto_connect,
        was_on: old.was_on,
    };
    store(&app, &s)?;
    let running = app.state::<Vpn>().tunnel.lock().await.as_ref().map(|t| t.socks_addr());
    if let Some(socks) = running {
        let reconnect = old.endpoint != s.endpoint || old.peer_public_key != s.peer_public_key || old.address != s.address || old.dns != s.dns || settings.preshared_key.is_some();
        if reconnect {
            start(&app).await?;
        } else {
            apply_route(&s, Some(socks));
        }
        changed(&app, true);
    }
    Ok(info(&app).await)
}

/// Makes a new key pair for this device; returns the public key for the server's [Peer].
#[tauri::command]
pub async fn vpn_generate_key(app: AppHandle) -> CmdResult<Info> {
    let k = rad_tunnel::generate_private_key();
    secrets::set_named(&dir(&app), KEY, &k).map_err(|e| CmdError::new("vpn", e.to_string()))?;
    Ok(info(&app).await)
}

/// Imports a `wg-quick` file (from the server's setup script, or `wg` tools).
#[tauri::command]
pub async fn vpn_import(app: AppHandle, text: String) -> CmdResult<Info> {
    let c = WgConfig::parse_wg_quick(&text).map_err(|e| CmdError::new("bad_request", format!("That isn't a usable WireGuard config: {e:#}")))?;
    let d = dir(&app);
    secrets::set_named(&d, KEY, &c.private_key).map_err(|e| CmdError::new("vpn", e.to_string()))?;
    match &c.preshared_key {
        Some(p) => secrets::set_named(&d, PSK, p).map_err(|e| CmdError::new("vpn", e.to_string()))?,
        None => secrets::delete_named(&d, PSK),
    }
    let old = load(&app);
    let s = Settings { endpoint: c.endpoint, peer_public_key: c.peer_public_key, address: c.address, dns: c.dns, keepalive: c.keepalive, mtu: c.mtu, ..old };
    store(&app, &s)?;
    Ok(info(&app).await)
}

/// Imports a `wg-quick` file picked from disk.
#[tauri::command]
pub async fn vpn_import_file(app: AppHandle, path: String) -> CmdResult<Info> {
    let text = std::fs::read_to_string(&path).map_err(|e| CmdError::new("bad_request", format!("Couldn't read {path}: {e}")))?;
    vpn_import(app, text).await
}

#[derive(Serialize)]
pub struct IpCheck {
    direct: Option<String>,
    vpn: Option<String>,
    error: Option<String>,
}

/// The public IP address the internet sees, directly and through the VPN.
#[tauri::command]
pub async fn vpn_check_ip(app: AppHandle) -> CmdResult<IpCheck> {
    const URL: &str = "https://api.ipify.org";
    let get = |proxy: Option<String>| async move {
        let mut b = reqwest::Client::builder().no_proxy().timeout(std::time::Duration::from_secs(15));
        if let Some(p) = proxy {
            b = b.proxy(reqwest::Proxy::all(p).map_err(|e| e.to_string())?);
        }
        let c = b.build().map_err(|e| e.to_string())?;
        let r = c.get(URL).send().await.map_err(|e| e.to_string())?;
        r.text().await.map(|t| t.trim().to_string()).map_err(|e| e.to_string())
    };
    let socks = app.state::<Vpn>().tunnel.lock().await.as_ref().map(|t| t.socks_addr());
    let direct = get(None).await.ok();
    let (vpn, error) = match socks {
        Some(s) => match get(Some(format!("socks5h://{s}"))).await {
            Ok(ip) => (Some(ip), None),
            Err(e) => (None, Some(e)),
        },
        None => (None, None),
    };
    Ok(IpCheck { direct, vpn, error })
}

/// Turns the VPN back on at launch if the user asked for that and it was on.
pub fn restore(app: &AppHandle) {
    let s = load(app);
    if s.auto_connect && s.was_on {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            match start(&app).await {
                Ok(()) => changed(&app, true),
                Err(e) => tracing::warn!("VPN didn't start: {}", e.message),
            }
        });
    }
}
