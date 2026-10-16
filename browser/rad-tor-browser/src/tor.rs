//! Arti (Tor in Rust) embedded in the browser engine.
//!
//! This starts a Tor client and a small HTTP **CONNECT** proxy on `127.0.0.1:0`. Servo is then
//! pointed at it through its `network_http_proxy_uri` / `network_https_proxy_uri` preferences.
//!
//! Servo's proxy connector (servo-net's `ProxyConnector`) only speaks HTTP CONNECT — it sends
//! `CONNECT host:port HTTP/1.1` for both http and https destinations — so the proxy here answers
//! that directly: it connects to the target over the Tor network and relays bytes. Hostnames are
//! resolved on the Tor side, so there is no DNS leak.
//!
//! Tor state and caches live in temporary directories that are deleted when the engine exits, so
//! nothing is kept between sessions.

use std::io;
use std::sync::Arc;
use std::time::Duration;

use arti_client::config::TorClientConfigBuilder;
use arti_client::{BootstrapBehavior, IntoTorAddr as _, StreamPrefs, TorClient};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tor_rtcompat::PreferredRuntime;

use crate::Msg;

type Client = TorClient<PreferredRuntime>;

/// The embedded Tor client and its HTTP CONNECT proxy. Dropped when the engine process exits.
pub(crate) struct Tor {
    _runtime: tokio::runtime::Runtime,
    _client: Arc<Client>,
    _state: tempfile::TempDir,
    _cache: tempfile::TempDir,
    /// The loopback proxy port Servo must be pointed at.
    pub(crate) proxy_port: u16,
}

impl Tor {
    /// Starts the Tor client and proxy. Bootstrap happens in the background; progress is
    /// reported back to the app over `tx` as `{"type":"tor", ...}` events.
    pub(crate) fn start(tx: crossbeam_channel::Sender<Msg>) -> Result<Tor, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;

        let state = tempfile::tempdir().map_err(|e| e.to_string())?;
        let cache = tempfile::tempdir().map_err(|e| e.to_string())?;
        let state_dir = state.path().to_path_buf();
        let cache_dir = cache.path().to_path_buf();

        let (client, proxy_port) = runtime.block_on(async move {
            let config = TorClientConfigBuilder::from_directories(&state_dir, &cache_dir)
                .build()
                .map_err(|e| e.to_string())?;
            let client = TorClient::builder()
                .config(config)
                .bootstrap_behavior(BootstrapBehavior::OnDemand)
                .create_unbootstrapped()
                .map_err(|e| e.to_string())?;

            let listener = TcpListener::bind(("127.0.0.1", 0))
                .await
                .map_err(|e| e.to_string())?;
            let proxy_port = listener.local_addr().map_err(|e| e.to_string())?.port();

            // Kick off bootstrap (it is also triggered on first use, but this makes sure it
            // starts even if no page has been requested yet).
            let boot = client.clone();
            let boot_tx = tx.clone();
            tokio::spawn(async move {
                match boot.bootstrap().await {
                    Ok(()) => {
                        let _ = boot_tx.send(Msg::Tor(json!({"type": "tor", "status": "ready"})));
                    }
                    Err(e) => {
                        let _ = boot_tx.send(Msg::Tor(json!({"type": "tor", "status": "error", "message": e.to_string()})));
                    }
                }
            });

            // Report bootstrap progress until the client is ready for traffic.
            let status = client.clone();
            let status_tx = tx.clone();
            tokio::spawn(async move {
                loop {
                    let st = status.bootstrap_status();
                    let ready = st.ready_for_traffic();
                    let _ = status_tx.send(Msg::Tor(json!({
                        "type": "tor",
                        "status": if ready { "ready" } else { "bootstrapping" },
                        "progress": st.as_frac(),
                    })));
                    if ready {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(400)).await;
                }
            });

            // Accept and relay CONNECT requests.
            let proxy = client.clone();
            tokio::spawn(async move { run_proxy(proxy, listener).await });

            Ok::<_, String>((client, proxy_port))
        })?;

        Ok(Tor { _runtime: runtime, _client: client, _state: state, _cache: cache, proxy_port })
    }
}

async fn run_proxy(client: Arc<Client>, listener: TcpListener) {
    loop {
        let (stream, _addr) = match listener.accept().await {
            Ok(pair) => pair,
            Err(_) => {
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };
        let client = client.clone();
        tokio::spawn(async move {
            let _ = handle_conn(client, stream).await;
        });
    }
}

async fn handle_conn(client: Arc<Client>, mut stream: TcpStream) -> io::Result<()> {
    // Read the request head (method line + headers, ending in a blank line).
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let header_end = loop {
        if let Some(pos) = find_header_end(&buf) {
            break pos;
        }
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            return Ok(()); // client hung up before finishing the request
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.len() > 64 * 1024 {
            let _ = stream.write_all(b"HTTP/1.1 431 Request Header Fields Too Large\r\nContent-Length: 0\r\n\r\n").await;
            return Ok(());
        }
    };

    // Parse "CONNECT host:port HTTP/1.1".
    let request_line = buf.split(|&b| b == b'\n').next().unwrap_or(&[]);
    let parts: Vec<&[u8]> = request_line.split(|&b| b == b' ').filter(|s| !s.is_empty()).collect();
    if parts.len() < 2 || parts[0] != b"CONNECT" {
        let _ = stream.write_all(b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\n\r\n").await;
        return Ok(());
    }
    let authority = String::from_utf8_lossy(parts[1]).into_owned();
    let Some((host, port)) = parse_authority(&authority) else {
        let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n").await;
        return Ok(());
    };

    // Connect to the destination over Tor. Hostnames are resolved on the Tor side.
    let prefs = StreamPrefs::new();
    let tor_addr = (host.as_str(), port).into_tor_addr().map_err(io_err)?;
    let mut tor_stream = match client.connect_with_prefs(&tor_addr, &prefs).await {
        Ok(s) => s,
        Err(_) => {
            let _ = stream.write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n").await;
            return Ok(());
        }
    };

    // Tell the client the tunnel is up.
    stream.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await?;
    stream.flush().await?;

    // Any bytes the client already sent after the headers go to the destination first.
    let leftover = &buf[header_end..];
    if !leftover.is_empty() {
        tor_stream.write_all(leftover).await?;
        tor_stream.flush().await?;
    }

    // Relay both ways until either side closes.
    let _ = tokio::io::copy_bidirectional(&mut stream, &mut tor_stream).await;
    Ok(())
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}

/// Splits a CONNECT authority into `(host, port)`: `host:port`, `[ipv6]:port`, or `host`.
fn parse_authority(authority: &str) -> Option<(String, u16)> {
    if let Some(rest) = authority.strip_prefix('[') {
        let end = rest.find(']')?;
        let port = rest[end + 1..].strip_prefix(':')?.parse().ok()?;
        Some((rest[..end].to_string(), port))
    } else if let Some((host, port)) = authority.rsplit_once(':') {
        Some((host.to_string(), port.parse().ok()?))
    } else {
        Some((authority.to_string(), 443))
    }
}

fn io_err<E: std::fmt::Display>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e.to_string())
}
