//! The tunnel engine: one task owning the UDP socket to the server, the WireGuard session
//! (boringtun) and a TCP/IP stack (smoltcp) whose packets go through that session.
//! Callers ask it for TCP connections and get back a pair of byte channels.

use std::collections::{HashMap, VecDeque};
use std::io;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use boringtun::noise::{Tunn, TunnResult};
use boringtun::x25519::{PublicKey, StaticSecret};
use smoltcp::iface::{Config as IfConfig, Interface, SocketHandle, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::{dns, tcp};
use smoltcp::time::Instant as SInstant;
use smoltcp::wire::{DnsQueryType, HardwareAddress, IpAddress, IpCidr};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot, Notify};
use tokio::time::Instant;

use crate::config::{decode_key, parse_cidr, WgConfig};
use crate::Status;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const TCP_BUF: usize = 256 * 1024;
const CHUNK: usize = 16 * 1024;
/// Data chunks queued per connection and direction before the other side waits.
const QUEUE: usize = 16;

/// Where a connection goes.
pub(crate) enum Dest {
    Ip(IpAddr, u16),
    Name(String, u16),
}

/// The two halves of a tunnelled TCP connection: bytes to send, bytes received (closed on EOF).
pub(crate) struct ConnIo {
    pub up: mpsc::Sender<Vec<u8>>,
    pub down: mpsc::Receiver<Vec<u8>>,
    /// Poke after sending or receiving, so the engine moves the next bytes.
    pub wake: Arc<Notify>,
}

pub(crate) enum Cmd {
    Connect { dest: Dest, reply: oneshot::Sender<io::Result<ConnIo>> },
}

// ------------------------------------------------------------------ virtual device

struct VDev {
    rx: VecDeque<Vec<u8>>,
    tx: VecDeque<Vec<u8>>,
    mtu: usize,
}

struct VRx(Vec<u8>);
struct VTx<'a>(&'a mut VecDeque<Vec<u8>>);

impl RxToken for VRx {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(&self.0)
    }
}

impl TxToken for VTx<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let mut b = vec![0; len];
        let r = f(&mut b);
        self.0.push_back(b);
        r
    }
}

impl Device for VDev {
    type RxToken<'a> = VRx where Self: 'a;
    type TxToken<'a> = VTx<'a> where Self: 'a;

    fn receive(&mut self, _: SInstant) -> Option<(VRx, VTx<'_>)> {
        let p = self.rx.pop_front()?;
        Some((VRx(p), VTx(&mut self.tx)))
    }

    fn transmit(&mut self, _: SInstant) -> Option<VTx<'_>> {
        Some(VTx(&mut self.tx))
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut c = DeviceCapabilities::default();
        c.medium = Medium::Ip;
        c.max_transmission_unit = self.mtu;
        c
    }
}

// ------------------------------------------------------------------ engine

struct Conn {
    up_rx: mpsc::Receiver<Vec<u8>>,
    pending: Vec<u8>,
    off: usize,
    up_closed: bool,
    fin_sent: bool,
    down_tx: Option<mpsc::Sender<Vec<u8>>>,
}

struct Connecting {
    reply: oneshot::Sender<io::Result<ConnIo>>,
    deadline: Instant,
}

struct Resolving {
    query: dns::QueryHandle,
    port: u16,
    reply: oneshot::Sender<io::Result<ConnIo>>,
    deadline: Instant,
}

pub(crate) struct Engine {
    udp: UdpSocket,
    tunn: Tunn,
    dev: VDev,
    iface: Interface,
    sockets: SocketSet<'static>,
    dns: Option<SocketHandle>,
    v4: bool,
    v6: bool,
    conns: HashMap<SocketHandle, Conn>,
    connecting: HashMap<SocketHandle, Connecting>,
    resolving: Vec<Resolving>,
    next_port: u16,
    wake: Arc<Notify>,
    status: Arc<Mutex<Status>>,
}

fn now() -> SInstant {
    SInstant::now()
}

fn to_ip(a: IpAddr) -> IpAddress {
    match a {
        IpAddr::V4(v) => IpAddress::Ipv4(v),
        IpAddr::V6(v) => IpAddress::Ipv6(v),
    }
}

impl Engine {
    pub(crate) async fn new(cfg: &WgConfig, wake: Arc<Notify>, status: Arc<Mutex<Status>>) -> anyhow::Result<Engine> {
        cfg.validate()?;
        let endpoint = tokio::net::lookup_host(cfg.endpoint.trim())
            .await
            .map_err(|e| anyhow::anyhow!("can't find the VPN server {}: {e}", cfg.endpoint))?
            .next()
            .ok_or_else(|| anyhow::anyhow!("can't find the VPN server {}", cfg.endpoint))?;
        let udp = UdpSocket::bind(if endpoint.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" }).await?;
        udp.connect(endpoint).await?;

        let secret = StaticSecret::from(decode_key(&cfg.private_key, "private key")?);
        let peer = PublicKey::from(decode_key(&cfg.peer_public_key, "server's public key")?);
        let psk = match cfg.preshared_key.as_deref().filter(|p| !p.trim().is_empty()) {
            Some(p) => Some(decode_key(p, "preshared key")?),
            None => None,
        };
        let mut idx = [0u8; 4];
        getrandom::getrandom(&mut idx)?;
        let tunn = Tunn::new(secret, peer, psk, cfg.keepalive.or(Some(25)), u32::from_le_bytes(idx) >> 8, None);

        let mtu = cfg.mtu.unwrap_or(1420) as usize;
        let mut dev = VDev { rx: VecDeque::new(), tx: VecDeque::new(), mtu };
        let mut ic = IfConfig::new(HardwareAddress::Ip);
        let mut seed = [0u8; 8];
        getrandom::getrandom(&mut seed)?;
        ic.random_seed = u64::from_le_bytes(seed);
        let mut iface = Interface::new(ic, &mut dev, now());
        let (mut v4, mut v6) = (false, false);
        let addrs: Vec<_> = cfg.address.iter().map(|a| parse_cidr(a)).collect::<anyhow::Result<_>>()?;
        iface.update_ip_addrs(|list| {
            for c in &addrs {
                let _ = list.push(IpCidr::new(to_ip(c.addr), c.prefix));
            }
        });
        for c in &addrs {
            // Point-to-point: every destination goes out through the tunnel.
            match c.addr {
                IpAddr::V4(a) if !v4 => {
                    v4 = true;
                    let _ = iface.routes_mut().add_default_ipv4_route(a);
                }
                IpAddr::V6(a) if !v6 => {
                    v6 = true;
                    let _ = iface.routes_mut().add_default_ipv6_route(a);
                }
                _ => {}
            }
        }

        let mut sockets = SocketSet::new(Vec::new());
        let servers: Vec<IpAddress> = cfg.dns.iter().filter_map(|d| d.trim().parse::<IpAddr>().ok()).map(to_ip).collect();
        let dns = (!servers.is_empty()).then(|| sockets.add(dns::Socket::new(&servers, (0..16).map(|_| None).collect::<Vec<_>>())));

        Ok(Engine {
            udp,
            tunn,
            dev,
            iface,
            sockets,
            dns,
            v4,
            v6,
            conns: HashMap::new(),
            connecting: HashMap::new(),
            resolving: Vec::new(),
            next_port: 49152,
            wake,
            status,
        })
    }

    pub(crate) async fn run(mut self, mut cmds: mpsc::Receiver<Cmd>) {
        let mut buf = vec![0u8; 65_536];
        let mut out = vec![0u8; 65_536 + 256];
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // Say hello straight away, so the status shows whether the server answers.
        if let TunnResult::WriteToNetwork(p) = self.tunn.format_handshake_initiation(&mut out, false) {
            let _ = self.udp.send(p).await;
        }
        loop {
            let delay = self
                .iface
                .poll_delay(now(), &self.sockets)
                .map(|d| Duration::from_micros(d.total_micros()))
                .unwrap_or(Duration::from_millis(250))
                .min(Duration::from_millis(250));
            tokio::select! {
                r = self.udp.recv(&mut buf) => match r {
                    Ok(n) => self.on_datagram(&buf[..n], &mut out).await,
                    Err(e) => self.set_error(format!("network: {e}")),
                },
                c = cmds.recv() => match c {
                    Some(Cmd::Connect { dest, reply }) => self.connect(dest, reply),
                    None => break,
                },
                _ = self.wake.notified() => {}
                _ = tokio::time::sleep(delay) => {}
                _ = tick.tick() => self.timers(&mut out).await,
            }
            self.pump(&mut out).await;
        }
    }

    fn set_error(&self, e: String) {
        self.status.lock().unwrap().last_error = Some(e);
    }

    async fn send_net(&self, p: &[u8]) {
        if let Err(e) = self.udp.send(p).await {
            self.set_error(format!("network: {e}"));
        }
    }

    async fn on_datagram(&mut self, data: &[u8], out: &mut [u8]) {
        match self.tunn.decapsulate(None, data, out) {
            TunnResult::WriteToNetwork(p) => {
                let p = p.to_vec();
                self.send_net(&p).await;
                // Packets waiting for the handshake go out now.
                loop {
                    match self.tunn.decapsulate(None, &[], out) {
                        TunnResult::WriteToNetwork(p) => {
                            let p = p.to_vec();
                            self.send_net(&p).await;
                        }
                        _ => break,
                    }
                }
            }
            TunnResult::WriteToTunnelV4(p, _) | TunnResult::WriteToTunnelV6(p, _) => self.dev.rx.push_back(p.to_vec()),
            TunnResult::Err(e) => self.set_error(format!("WireGuard: {e:?}")),
            TunnResult::Done => {}
        }
    }

    async fn timers(&mut self, out: &mut [u8]) {
        match self.tunn.update_timers(out) {
            TunnResult::WriteToNetwork(p) => {
                let p = p.to_vec();
                self.send_net(&p).await;
            }
            TunnResult::Err(e) => self.set_error(format!("WireGuard: {e:?}")),
            _ => {}
        }
        let (since, tx, rx, _, _) = self.tunn.stats();
        let mut s = self.status.lock().unwrap();
        s.handshake_ago_secs = since.map(|d| d.as_secs());
        s.up = since.is_some_and(|d| d < Duration::from_secs(180));
        if s.up {
            s.last_error = None;
        }
        s.tx_bytes = tx as u64;
        s.rx_bytes = rx as u64;
        s.connections = self.conns.len();
    }

    fn connect(&mut self, dest: Dest, reply: oneshot::Sender<io::Result<ConnIo>>) {
        match dest {
            Dest::Ip(ip, port) => self.open_tcp(ip, port, reply),
            Dest::Name(name, port) => {
                if let Ok(ip) = name.trim_matches(['[', ']']).parse::<IpAddr>() {
                    return self.open_tcp(ip, port, reply);
                }
                let Some(h) = self.dns else {
                    let _ = reply.send(Err(io::Error::other("the VPN has no DNS server set; add DNS = 10.66.0.1")));
                    return;
                };
                let qtype = if self.v4 { DnsQueryType::A } else { DnsQueryType::Aaaa };
                let cx = self.iface.context();
                match self.sockets.get_mut::<dns::Socket>(h).start_query(cx, &name, qtype) {
                    Ok(query) => self.resolving.push(Resolving { query, port, reply, deadline: Instant::now() + CONNECT_TIMEOUT }),
                    Err(e) => {
                        let _ = reply.send(Err(io::Error::new(io::ErrorKind::InvalidInput, format!("can't look up {name}: {e:?}"))));
                    }
                }
            }
        }
    }

    fn open_tcp(&mut self, ip: IpAddr, port: u16, reply: oneshot::Sender<io::Result<ConnIo>>) {
        if (ip.is_ipv4() && !self.v4) || (ip.is_ipv6() && !self.v6) {
            let _ = reply.send(Err(io::Error::new(io::ErrorKind::Unsupported, format!("the VPN has no {} address", if ip.is_ipv4() { "IPv4" } else { "IPv6" }))));
            return;
        }
        let mut s = tcp::Socket::new(tcp::SocketBuffer::new(vec![0; TCP_BUF]), tcp::SocketBuffer::new(vec![0; TCP_BUF]));
        s.set_nagle_enabled(false);
        s.set_keep_alive(Some(smoltcp::time::Duration::from_secs(30)));
        let local = self.next_port;
        self.next_port = if self.next_port == u16::MAX { 49152 } else { self.next_port + 1 };
        let cx = self.iface.context();
        if let Err(e) = s.connect(cx, (to_ip(ip), port), local) {
            let _ = reply.send(Err(io::Error::other(format!("connect: {e:?}"))));
            return;
        }
        let h = self.sockets.add(s);
        self.connecting.insert(h, Connecting { reply, deadline: Instant::now() + CONNECT_TIMEOUT });
    }

    /// Runs the stack until nothing moves: packets in, sockets serviced, packets out.
    async fn pump(&mut self, out: &mut [u8]) {
        for _ in 0..16 {
            self.iface.poll(now(), &mut self.dev, &mut self.sockets);
            let sent = !self.dev.tx.is_empty();
            while let Some(pkt) = self.dev.tx.pop_front() {
                match self.tunn.encapsulate(&pkt, out) {
                    TunnResult::WriteToNetwork(p) => {
                        let p = p.to_vec();
                        self.send_net(&p).await;
                    }
                    TunnResult::Err(e) => self.set_error(format!("WireGuard: {e:?}")),
                    _ => {}
                }
            }
            let moved = self.service();
            if !moved && !sent && self.dev.rx.is_empty() {
                break;
            }
        }
    }

    /// Moves bytes between sockets and channels; settles connects and lookups. True if anything moved.
    fn service(&mut self) -> bool {
        let mut moved = false;
        let t = Instant::now();

        // Lookups.
        if let Some(h) = self.dns {
            let mut i = 0;
            while i < self.resolving.len() {
                let r = self.sockets.get_mut::<dns::Socket>(h).get_query_result(self.resolving[i].query);
                let timed_out = t > self.resolving[i].deadline;
                match r {
                    Err(dns::GetQueryResultError::Pending) if !timed_out => i += 1,
                    other => {
                        let job = self.resolving.swap_remove(i);
                        moved = true;
                        let ip = other.ok().and_then(|ips| ips.into_iter().next());
                        match ip {
                            Some(IpAddress::Ipv4(a)) => self.open_tcp(IpAddr::V4(a), job.port, job.reply),
                            Some(IpAddress::Ipv6(a)) => self.open_tcp(IpAddr::V6(a), job.port, job.reply),
                            None => {
                                let _ = job.reply.send(Err(io::Error::new(io::ErrorKind::NotFound, "name not found (DNS through the VPN)")));
                            }
                        }
                    }
                }
            }
        }

        // Connects.
        let pending: Vec<SocketHandle> = self.connecting.keys().copied().collect();
        for h in pending {
            let state = self.sockets.get::<tcp::Socket>(h).state();
            let late = t > self.connecting[&h].deadline;
            if state == tcp::State::Established {
                let c = self.connecting.remove(&h).unwrap();
                let (up_tx, up_rx) = mpsc::channel(QUEUE);
                let (down_tx, down_rx) = mpsc::channel(QUEUE);
                if c.reply.send(Ok(ConnIo { up: up_tx, down: down_rx, wake: self.wake.clone() })).is_ok() {
                    self.conns.insert(h, Conn { up_rx, pending: Vec::new(), off: 0, up_closed: false, fin_sent: false, down_tx: Some(down_tx) });
                } else {
                    self.sockets.get_mut::<tcp::Socket>(h).abort();
                    self.conns.insert(h, Conn { up_rx, pending: Vec::new(), off: 0, up_closed: true, fin_sent: true, down_tx: None });
                }
                moved = true;
            } else if state == tcp::State::Closed || late {
                let c = self.connecting.remove(&h).unwrap();
                self.sockets.remove(h);
                let e = if late {
                    io::Error::new(io::ErrorKind::TimedOut, "no answer through the VPN")
                } else {
                    io::Error::new(io::ErrorKind::ConnectionRefused, "connection refused")
                };
                let _ = c.reply.send(Err(e));
                moved = true;
            }
        }

        // Open connections.
        let mut done = Vec::new();
        for (&h, c) in self.conns.iter_mut() {
            let s = self.sockets.get_mut::<tcp::Socket>(h);
            // Up: channel -> socket.
            loop {
                if c.off >= c.pending.len() {
                    if c.up_closed {
                        break;
                    }
                    match c.up_rx.try_recv() {
                        Ok(v) => {
                            c.pending = v;
                            c.off = 0;
                        }
                        Err(mpsc::error::TryRecvError::Empty) => break,
                        Err(mpsc::error::TryRecvError::Disconnected) => {
                            c.up_closed = true;
                            break;
                        }
                    }
                }
                if !s.can_send() {
                    break;
                }
                let n = s.send_slice(&c.pending[c.off..]).unwrap_or(0);
                if n == 0 {
                    break;
                }
                c.off += n;
                moved = true;
            }
            if c.up_closed && c.off >= c.pending.len() && !c.fin_sent {
                s.close();
                c.fin_sent = true;
                moved = true;
            }
            // Down: socket -> channel.
            if let Some(tx) = &c.down_tx {
                while s.can_recv() {
                    match tx.try_reserve() {
                        Ok(permit) => {
                            let mut b = vec![0; CHUNK];
                            let n = s.recv_slice(&mut b).unwrap_or(0);
                            b.truncate(n);
                            permit.send(b);
                            moved = true;
                        }
                        Err(mpsc::error::TrySendError::Full(_)) => break,
                        Err(mpsc::error::TrySendError::Closed(_)) => {
                            // The local side went away: drop the connection.
                            s.abort();
                            c.up_closed = true;
                            c.fin_sent = true;
                            break;
                        }
                    }
                }
                if !s.may_recv() && !s.can_recv() {
                    c.down_tx = None;
                    moved = true;
                }
            }
            if matches!(s.state(), tcp::State::Closed | tcp::State::TimeWait) && c.down_tx.is_none() {
                done.push(h);
            }
        }
        for h in done {
            self.conns.remove(&h);
            self.sockets.remove(h);
        }
        moved
    }
}

/// Resolves a SOCKS-style destination.
pub(crate) fn dest(host: &str, port: u16) -> Dest {
    match host.trim_matches(['[', ']']).parse::<IpAddr>() {
        Ok(ip) => Dest::Ip(ip, port),
        Err(_) => Dest::Name(host.to_string(), port),
    }
}
