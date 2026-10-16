//! WireGuard settings: the `wg-quick` file format, and this device's keys.

use std::net::IpAddr;

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine as _;
use boringtun::x25519::{PublicKey, StaticSecret};
use serde::{Deserialize, Serialize};

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// One WireGuard tunnel to one server (a `wg-quick` file with one `[Peer]`). Keys are base64.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct WgConfig {
    pub private_key: String,
    pub peer_public_key: String,
    #[serde(default)]
    pub preshared_key: Option<String>,
    /// `host:port` of the server.
    pub endpoint: String,
    /// This device's tunnel addresses, e.g. `10.66.0.2/32`, `fd66::2/128`.
    pub address: Vec<String>,
    /// DNS servers reached through the tunnel, e.g. `10.66.0.1`.
    #[serde(default)]
    pub dns: Vec<String>,
    #[serde(default)]
    pub keepalive: Option<u16>,
    #[serde(default)]
    pub mtu: Option<u16>,
}

/// A parsed `address` entry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cidr {
    pub addr: IpAddr,
    pub prefix: u8,
}

pub fn parse_cidr(s: &str) -> Result<Cidr> {
    let s = s.trim();
    let (a, p) = s.split_once('/').unwrap_or((s, ""));
    let addr: IpAddr = a.trim().parse().with_context(|| format!("“{a}” isn't an IP address"))?;
    let max = if addr.is_ipv4() { 32 } else { 128 };
    let prefix = if p.is_empty() { max } else { p.trim().parse::<u8>().ok().filter(|p| *p <= max).ok_or_else(|| anyhow!("bad prefix in “{s}”"))? };
    Ok(Cidr { addr, prefix })
}

impl Cidr {
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.addr, ip) {
            (IpAddr::V4(n), IpAddr::V4(i)) => {
                let m = if self.prefix == 0 { 0 } else { u32::MAX << (32 - self.prefix) };
                u32::from(n) & m == u32::from(i) & m
            }
            (IpAddr::V6(n), IpAddr::V6(i)) => {
                let m = if self.prefix == 0 { 0 } else { u128::MAX << (128 - self.prefix) };
                u128::from(n) & m == u128::from(i) & m
            }
            _ => false,
        }
    }
}

pub fn decode_key(b64: &str, what: &str) -> Result<[u8; 32]> {
    let v = B64.decode(b64.trim()).with_context(|| format!("the {what} isn't valid base64"))?;
    v.try_into().map_err(|_| anyhow!("the {what} must be 32 bytes"))
}

/// A new private key for this device (base64).
pub fn generate_private_key() -> String {
    let mut k = [0u8; 32];
    getrandom::getrandom(&mut k).expect("system random generator");
    // Clamp as wg(8) does, so the key is identical wherever it's used.
    k[0] &= 248;
    k[31] &= 127;
    k[31] |= 64;
    B64.encode(k)
}

/// The public key (base64) that goes into the server's `[Peer]` for this device.
pub fn public_key(private_b64: &str) -> Result<String> {
    let secret = StaticSecret::from(decode_key(private_b64, "private key")?);
    Ok(B64.encode(PublicKey::from(&secret).as_bytes()))
}

impl WgConfig {
    /// Checks every field; returns a message a person can act on.
    pub fn validate(&self) -> Result<()> {
        decode_key(&self.private_key, "private key")?;
        decode_key(&self.peer_public_key, "server's public key")?;
        if let Some(p) = self.preshared_key.as_deref().filter(|p| !p.trim().is_empty()) {
            decode_key(p, "preshared key")?;
        }
        if !self.endpoint.contains(':') {
            bail!("the server endpoint needs a port, e.g. vpn.example.com:51820");
        }
        if self.address.is_empty() {
            bail!("give this device a tunnel address, e.g. 10.66.0.2/32");
        }
        for a in &self.address {
            parse_cidr(a)?;
        }
        for d in &self.dns {
            d.trim().parse::<IpAddr>().with_context(|| format!("DNS “{d}” isn't an IP address"))?;
        }
        Ok(())
    }

    /// Reads a `wg-quick` file (the `[Interface]` and the first `[Peer]`).
    pub fn parse_wg_quick(text: &str) -> Result<WgConfig> {
        let mut c = WgConfig::default();
        let mut section = String::new();
        let mut peers = 0;
        for raw in text.lines() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            if line.starts_with('[') {
                section = line.trim_matches(['[', ']']).to_ascii_lowercase();
                if section == "peer" {
                    peers += 1;
                }
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
            let list = || v.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect::<Vec<_>>();
            match (section.as_str(), k.as_str()) {
                ("interface", "privatekey") => c.private_key = v.into(),
                ("interface", "address") => c.address.extend(list()),
                // Search domains can sit in DNS too; keep only the servers.
                ("interface", "dns") => c.dns.extend(list().into_iter().filter(|d| d.parse::<IpAddr>().is_ok())),
                ("interface", "mtu") => c.mtu = v.parse().ok(),
                ("peer", _) if peers > 1 => {}
                ("peer", "publickey") => c.peer_public_key = v.into(),
                ("peer", "presharedkey") => c.preshared_key = Some(v.into()),
                ("peer", "endpoint") => c.endpoint = v.into(),
                ("peer", "persistentkeepalive") => c.keepalive = v.parse().ok().filter(|k| *k > 0),
                _ => {}
            }
        }
        if peers == 0 {
            bail!("no [Peer] section — is this a WireGuard config?");
        }
        c.validate()?;
        Ok(c)
    }

    /// The file `wg-quick` would read, for showing or exporting. Contains the private key.
    pub fn to_wg_quick(&self) -> String {
        let mut s = format!("[Interface]\nPrivateKey = {}\nAddress = {}\n", self.private_key, self.address.join(", "));
        if !self.dns.is_empty() {
            s += &format!("DNS = {}\n", self.dns.join(", "));
        }
        if let Some(m) = self.mtu {
            s += &format!("MTU = {m}\n");
        }
        s += &format!("\n[Peer]\nPublicKey = {}\n", self.peer_public_key);
        if let Some(p) = &self.preshared_key {
            s += &format!("PresharedKey = {p}\n");
        }
        s += &format!("Endpoint = {}\nAllowedIPs = 0.0.0.0/0, ::/0\n", self.endpoint);
        if let Some(k) = self.keepalive {
            s += &format!("PersistentKeepalive = {k}\n");
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_wg_quick_and_round_trips_keys() {
        let private = generate_private_key();
        let server = public_key(&generate_private_key()).unwrap();
        let text = format!(
            "[Interface]\nPrivateKey = {private}\nAddress = 10.66.0.2/32, fd66::2/128\nDNS = 10.66.0.1, home.lan\n\n[Peer]\n# the server\nPublicKey = {server}\nEndpoint = vpn.example.com:51820\nAllowedIPs = 0.0.0.0/0, ::/0\nPersistentKeepalive = 25\n"
        );
        let c = WgConfig::parse_wg_quick(&text).unwrap();
        assert_eq!(c.address, vec!["10.66.0.2/32", "fd66::2/128"]);
        assert_eq!(c.dns, vec!["10.66.0.1"]);
        assert_eq!(c.keepalive, Some(25));
        assert_eq!(WgConfig::parse_wg_quick(&c.to_wg_quick()).unwrap(), c);
    }

    #[test]
    fn cidr_contains() {
        let c = parse_cidr("10.66.0.2/24").unwrap();
        assert!(c.contains("10.66.0.1".parse().unwrap()));
        assert!(!c.contains("10.67.0.1".parse().unwrap()));
    }
}
