//! A minimal SOCKS5 server (RFC 1928, CONNECT only) on 127.0.0.1 in front of the tunnel.
//! Names are resolved by the tunnel's DNS (`socks5h`), so lookups don't leak either.

use std::io;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};

use crate::engine::{dest, Cmd, ConnIo};

pub(crate) async fn serve(listener: TcpListener, cmds: mpsc::Sender<Cmd>) {
    let cmds = Arc::new(cmds);
    while let Ok((stream, peer)) = listener.accept().await {
        if !peer.ip().is_loopback() {
            continue;
        }
        let cmds = cmds.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(stream, &cmds).await {
                tracing::debug!(target: "rad_tunnel", "socks: {e}");
            }
        });
    }
}

async fn handle(mut s: TcpStream, cmds: &mpsc::Sender<Cmd>) -> io::Result<()> {
    s.set_nodelay(true)?;
    // Greeting: only "no authentication" (the port is loopback-only and random).
    let mut h = [0u8; 2];
    s.read_exact(&mut h).await?;
    if h[0] != 5 {
        return Err(io::Error::other("not SOCKS5"));
    }
    let mut methods = vec![0u8; h[1] as usize];
    s.read_exact(&mut methods).await?;
    if !methods.contains(&0) {
        s.write_all(&[5, 0xff]).await?;
        return Ok(());
    }
    s.write_all(&[5, 0]).await?;

    // Request.
    let mut r = [0u8; 4];
    s.read_exact(&mut r).await?;
    if r[1] != 1 {
        reply(&mut s, 7).await?; // command not supported
        return Ok(());
    }
    let host = match r[3] {
        1 => {
            let mut a = [0u8; 4];
            s.read_exact(&mut a).await?;
            std::net::Ipv4Addr::from(a).to_string()
        }
        3 => {
            let n = s.read_u8().await? as usize;
            let mut name = vec![0u8; n];
            s.read_exact(&mut name).await?;
            String::from_utf8(name).map_err(|_| io::Error::other("bad name"))?
        }
        4 => {
            let mut a = [0u8; 16];
            s.read_exact(&mut a).await?;
            std::net::Ipv6Addr::from(a).to_string()
        }
        _ => {
            reply(&mut s, 8).await?;
            return Ok(());
        }
    };
    let port = s.read_u16().await?;

    let (tx, rx) = oneshot::channel();
    if cmds.send(Cmd::Connect { dest: dest(&host, port), reply: tx }).await.is_err() {
        reply(&mut s, 1).await?;
        return Ok(());
    }
    let io = match rx.await.unwrap_or_else(|_| Err(io::Error::other("VPN stopped"))) {
        Ok(io) => io,
        Err(e) => {
            let code = match e.kind() {
                io::ErrorKind::ConnectionRefused => 5,
                io::ErrorKind::TimedOut => 4,
                io::ErrorKind::NotFound => 4,
                io::ErrorKind::Unsupported => 8,
                _ => 1,
            };
            reply(&mut s, code).await?;
            return Ok(());
        }
    };
    reply(&mut s, 0).await?;
    splice(s, io).await;
    Ok(())
}

async fn reply(s: &mut TcpStream, code: u8) -> io::Result<()> {
    s.write_all(&[5, code, 0, 1, 0, 0, 0, 0, 0, 0]).await
}

/// Copies bytes both ways between a local socket and a tunnelled connection.
pub(crate) async fn splice(s: TcpStream, io: ConnIo) {
    let ConnIo { up, mut down, wake } = io;
    let (mut rd, mut wr) = s.into_split();
    let w1 = wake.clone();
    let upload = async move {
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            let n = match rd.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            if up.send(buf[..n].to_vec()).await.is_err() {
                break;
            }
            w1.notify_one();
        }
        drop(up);
        w1.notify_one();
    };
    let download = async move {
        while let Some(v) = down.recv().await {
            wake.notify_one();
            if wr.write_all(&v).await.is_err() {
                break;
            }
        }
        let _ = wr.shutdown().await;
        wake.notify_one();
    };
    tokio::join!(upload, download);
}
