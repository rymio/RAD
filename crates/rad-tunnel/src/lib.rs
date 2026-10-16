//! RAD's VPN: WireGuard in user space. The tunnel runs inside RAD (boringtun for WireGuard,
//! smoltcp for TCP/IP), so it needs no administrator rights, no driver and no OS VPN — the same
//! code on macOS, Windows and Linux. Only RAD's own traffic uses it, through a SOCKS5 proxy on
//! 127.0.0.1 (see [`route`] for how each kind of connection opts in).

pub mod config;
mod engine;
pub mod route;
mod socks;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, Notify};

pub use config::{generate_private_key, public_key, WgConfig};

/// What the tunnel is doing, for the UI.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Status {
    /// A handshake with the server succeeded in the last 3 minutes.
    pub up: bool,
    pub handshake_ago_secs: Option<u64>,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub connections: usize,
    pub last_error: Option<String>,
}

/// A running tunnel; stops when dropped.
pub struct Tunnel {
    socks: SocketAddr,
    status: Arc<Mutex<Status>>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Tunnel {
    /// Connects to the server and opens the local proxy. Fails fast on bad settings or an
    /// unknown server name; a server that doesn't answer shows in [`Tunnel::status`].
    pub async fn start(cfg: &WgConfig) -> anyhow::Result<Tunnel> {
        let status: Arc<Mutex<Status>> = Arc::default();
        let wake = Arc::new(Notify::new());
        let engine = engine::Engine::new(cfg, wake, status.clone()).await?;
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let socks = listener.local_addr()?;
        let (tx, rx) = mpsc::channel(256);
        let tasks = vec![tokio::spawn(engine.run(rx)), tokio::spawn(socks::serve(listener, tx))];
        Ok(Tunnel { socks, status, tasks })
    }

    /// The local SOCKS5 proxy (`127.0.0.1:<port>`).
    pub fn socks_addr(&self) -> SocketAddr {
        self.socks
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}
