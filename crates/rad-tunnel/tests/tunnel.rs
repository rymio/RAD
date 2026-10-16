//! End to end: a SOCKS client -> RAD's tunnel -> UDP -> a fake WireGuard server running its own
//! TCP/IP stack with an echo service on 10.66.0.1:7 and a DNS server answering `echo.test`.

use std::collections::VecDeque;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use boringtun::noise::{Tunn, TunnResult};
use boringtun::x25519::{PublicKey, StaticSecret};
use rad_tunnel::{generate_private_key, public_key, Tunnel, WgConfig};
use smoltcp::iface::{Config, Interface, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::{tcp, udp};
use smoltcp::time::Instant as SInstant;
use smoltcp::wire::{HardwareAddress, IpAddress, IpCidr};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UdpSocket;

const SERVER_IP: Ipv4Addr = Ipv4Addr::new(10, 66, 0, 1);

struct Dev {
    rx: VecDeque<Vec<u8>>,
    tx: VecDeque<Vec<u8>>,
}
struct Rx(Vec<u8>);
struct Tx<'a>(&'a mut VecDeque<Vec<u8>>);
impl RxToken for Rx {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(&self.0)
    }
}
impl TxToken for Tx<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let mut b = vec![0; len];
        let r = f(&mut b);
        self.0.push_back(b);
        r
    }
}
impl Device for Dev {
    type RxToken<'a> = Rx;
    type TxToken<'a> = Tx<'a>;
    fn receive(&mut self, _: SInstant) -> Option<(Rx, Tx<'_>)> {
        let p = self.rx.pop_front()?;
        Some((Rx(p), Tx(&mut self.tx)))
    }
    fn transmit(&mut self, _: SInstant) -> Option<Tx<'_>> {
        Some(Tx(&mut self.tx))
    }
    fn capabilities(&self) -> DeviceCapabilities {
        let mut c = DeviceCapabilities::default();
        c.medium = Medium::Ip;
        c.max_transmission_unit = 1420;
        c
    }
}

/// Answers any A query with SERVER_IP.
fn dns_answer(q: &[u8]) -> Option<Vec<u8>> {
    if q.len() < 12 {
        return None;
    }
    let mut i = 12;
    while i < q.len() && q[i] != 0 {
        i += 1 + q[i] as usize;
    }
    let qend = i + 5; // zero byte + type + class
    let mut r = Vec::from(&q[..2]);
    r.extend_from_slice(&[0x81, 0x80, 0, 1, 0, 1, 0, 0, 0, 0]);
    r.extend_from_slice(q.get(12..qend)?);
    r.extend_from_slice(&[0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4]);
    r.extend_from_slice(&SERVER_IP.octets());
    Some(r)
}

async fn fake_server(udp: UdpSocket, server_secret: StaticSecret, client_public: PublicKey) {
    let mut tunn = Tunn::new(server_secret, client_public, None, None, 7, None);
    let mut dev = Dev { rx: VecDeque::new(), tx: VecDeque::new() };
    let mut iface = Interface::new(Config::new(HardwareAddress::Ip), &mut dev, SInstant::now());
    iface.update_ip_addrs(|a| {
        a.push(IpCidr::new(IpAddress::Ipv4(SERVER_IP), 24)).unwrap();
    });
    let mut sockets = SocketSet::new(Vec::new());
    let mut listeners = Vec::new();
    for _ in 0..16 {
        let mut s = tcp::Socket::new(tcp::SocketBuffer::new(vec![0; 65536]), tcp::SocketBuffer::new(vec![0; 65536]));
        s.listen(7).unwrap();
        listeners.push(sockets.add(s));
    }
    let mut d = udp::Socket::new(
        udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; 8], vec![0; 4096]),
        udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; 8], vec![0; 4096]),
    );
    d.bind(53).unwrap();
    let dns = sockets.add(d);

    let mut client: Option<SocketAddr> = None;
    let mut buf = vec![0u8; 65536];
    let mut out = vec![0u8; 65536 + 256];
    loop {
        if let Ok(Ok((n, from))) = tokio::time::timeout(Duration::from_millis(5), udp.recv_from(&mut buf)).await {
            client = Some(from);
            match tunn.decapsulate(Some(from.ip()), &buf[..n], &mut out) {
                TunnResult::WriteToNetwork(p) => {
                    udp.send_to(p, from).await.unwrap();
                    while let TunnResult::WriteToNetwork(p) = tunn.decapsulate(None, &[], &mut out) {
                        udp.send_to(p, from).await.unwrap();
                    }
                }
                TunnResult::WriteToTunnelV4(p, _) => dev.rx.push_back(p.to_vec()),
                _ => {}
            }
        }
        for _ in 0..8 {
            iface.poll(SInstant::now(), &mut dev, &mut sockets);
            for &h in &listeners {
                let s = sockets.get_mut::<tcp::Socket>(h);
                while s.can_recv() && s.can_send() {
                    let mut b = vec![0u8; s.send_capacity() - s.send_queue()];
                    let n = s.recv_slice(&mut b).unwrap();
                    s.send_slice(&b[..n]).unwrap();
                }
                if !s.may_recv() && s.may_send() {
                    s.close();
                }
                if !s.is_open() {
                    s.listen(7).unwrap();
                }
            }
            let ds = sockets.get_mut::<udp::Socket>(dns);
            while let Ok((q, meta)) = ds.recv() {
                let (q, meta) = (q.to_vec(), meta);
                if let Some(a) = dns_answer(&q) {
                    ds.send_slice(&a, meta.endpoint).unwrap();
                }
            }
            if dev.tx.is_empty() {
                break;
            }
            while let Some(p) = dev.tx.pop_front() {
                if let (TunnResult::WriteToNetwork(w), Some(c)) = (tunn.encapsulate(&p, &mut out), client) {
                    udp.send_to(w, c).await.unwrap();
                }
            }
        }
        if let (TunnResult::WriteToNetwork(p), Some(c)) = (tunn.update_timers(&mut out), client) {
            udp.send_to(p, c).await.unwrap();
        }
    }
}

async fn echo_through(proxy: SocketAddr, host: &str, size: usize) {
    let mut s = tokio_socks::tcp::Socks5Stream::connect(proxy, (host, 7)).await.unwrap_or_else(|e| panic!("socks connect to {host}: {e:?}")).into_inner();
    let data: Vec<u8> = (0..size).map(|i| (i * 7 % 251) as u8).collect();
    let (mut rd, mut wr) = s.split();
    let send = async {
        wr.write_all(&data).await.unwrap();
        wr.shutdown().await.unwrap();
    };
    let mut got = Vec::new();
    let recv = async { rd.read_to_end(&mut got).await.unwrap() };
    tokio::join!(send, recv);
    assert_eq!(got.len(), data.len());
    assert!(got == data, "echoed bytes differ");
}

#[tokio::test(flavor = "multi_thread")]
async fn tcp_and_dns_through_the_tunnel() {
    let server_key = generate_private_key();
    let client_key = generate_private_key();
    let decode = |k: &str| -> [u8; 32] { rad_tunnel::config::decode_key(k, "key").unwrap() };
    let udp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let endpoint = udp.local_addr().unwrap();
    tokio::spawn(fake_server(udp, StaticSecret::from(decode(&server_key)), PublicKey::from(decode(&public_key(&client_key).unwrap()))));

    let cfg = WgConfig {
        private_key: client_key,
        peer_public_key: public_key(&server_key).unwrap(),
        endpoint: endpoint.to_string(),
        address: vec!["10.66.0.2/32".into()],
        dns: vec!["10.66.0.1".into()],
        ..Default::default()
    };
    let tunnel = Tunnel::start(&cfg).await.unwrap();
    let proxy = tunnel.socks_addr();

    tokio::time::timeout(Duration::from_secs(30), async {
        echo_through(proxy, "10.66.0.1", 300_000).await;
        echo_through(proxy, "echo.test", 50_000).await;
        // Several at once.
        let jobs: Vec<_> = (0..3).map(|_| tokio::spawn(echo_through(proxy, "10.66.0.1", 100_000))).collect();
        for j in jobs {
            j.await.unwrap();
        }
    })
    .await
    .expect("tunnel test timed out");

    tokio::time::sleep(Duration::from_millis(600)).await; // counters refresh every 250 ms
    let st = tunnel.status();
    assert!(st.up, "handshake should be done: {st:?}");
    assert!(st.tx_bytes > 500_000 && st.rx_bytes > 500_000, "{st:?}");
}
