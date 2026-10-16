//! Which of RAD's connections go through the VPN. Set once by the app when the tunnel starts or
//! stops; read by every place that opens a connection (HTTP clients, email, SSH, engines).

use std::net::{IpAddr, SocketAddr};
use std::sync::RwLock;

use crate::config::Cidr;

/// A kind of traffic the user can send through the VPN (or not).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// RAD server, AI providers, model and voice downloads.
    Services,
    /// The Chromium engine.
    Browsers,
    Email,
    Ssh,
    Rymnet,
}

#[derive(Debug, Clone)]
pub struct Route {
    pub socks: SocketAddr,
    pub services: bool,
    pub browsers: bool,
    pub email: bool,
    pub ssh: bool,
    pub rymnet: bool,
    /// The tunnel's own networks (e.g. 10.66.0.0/24): always through the tunnel, even if private.
    pub tunnel_nets: Vec<Cidr>,
}

static ROUTE: RwLock<Option<Route>> = RwLock::new(None);

/// Turns routing on (with the tunnel's proxy) or off.
pub fn set(route: Option<Route>) {
    *ROUTE.write().unwrap() = route;
}

pub fn current() -> Option<Route> {
    ROUTE.read().unwrap().clone()
}

/// The SOCKS5 proxy for this kind of traffic, while the VPN is on and covers it.
pub fn socks_for(scope: Scope) -> Option<SocketAddr> {
    let r = ROUTE.read().unwrap();
    let r = r.as_ref()?;
    let on = match scope {
        Scope::Services => r.services,
        Scope::Browsers => r.browsers,
        Scope::Email => r.email,
        Scope::Ssh => r.ssh,
        Scope::Rymnet => r.rymnet,
    };
    on.then_some(r.socks)
}

/// This computer and the local network stay direct (Ollama, a LAN server, the embedded vault),
/// except addresses inside the tunnel.
pub fn is_local(host: &str) -> bool {
    let h = host.trim_matches(['[', ']']).to_ascii_lowercase();
    if h == "localhost" || h.ends_with(".localhost") || h.ends_with(".local") {
        return true;
    }
    let Ok(ip) = h.parse::<IpAddr>() else { return false };
    if let Some(r) = ROUTE.read().unwrap().as_ref() {
        if r.tunnel_nets.iter().any(|n| n.contains(ip)) {
            return false;
        }
    }
    match ip {
        IpAddr::V4(v) => v.is_loopback() || v.is_private() || v.is_link_local() || v.is_unspecified(),
        IpAddr::V6(v) => v.is_loopback() || v.is_unspecified() || (v.segments()[0] & 0xfe00) == 0xfc00 || (v.segments()[0] & 0xffc0) == 0xfe80,
    }
}

/// A `reqwest` proxy that follows the VPN as it's turned on and off. Add it to every client:
/// `.proxy(rad_tunnel::route::reqwest_proxy())`.
pub fn reqwest_proxy() -> reqwest::Proxy {
    reqwest::Proxy::custom(|url| {
        let host = url.host_str()?;
        if is_local(host) {
            return None;
        }
        socks_for(Scope::Services).map(|s| format!("socks5h://{s}"))
    })
}

/// Opens a TCP connection for this kind of traffic: through the VPN when it covers it,
/// directly otherwise. The stream is a plain `TcpStream` either way.
pub async fn connect_tcp(scope: Scope, host: &str, port: u16) -> std::io::Result<tokio::net::TcpStream> {
    match socks_for(scope).filter(|_| !is_local(host)) {
        Some(proxy) => tokio_socks::tcp::Socks5Stream::connect(proxy, (host, port))
            .await
            .map(|s| s.into_inner())
            .map_err(|e| std::io::Error::other(format!("through the VPN: {e}"))),
        None => tokio::net::TcpStream::connect((host, port)).await,
    }
}
