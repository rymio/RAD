//! Web Monitoring: checks a website (its IP addresses, whether it answers and how fast, and
//! its TLS certificate: who issued it, the names it covers and when it expires) and takes a
//! thumbnail of it with a browser engine of its own — a separate process and profile, so the
//! user's tabs and sign-ins are never touched. The sites, their SSH details and notes are kept
//! in the vault by the UI (`app/ui/src/lib/monitor.svelte.ts`).

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tauri::ipc::Response;
use tauri::{AppHandle, Manager};
use tokio_rustls::rustls;

use crate::client::{CmdError, CmdResult};

const TIMEOUT: Duration = Duration::from_secs(20);
/// Of the page, to find its title.
const TITLE_BYTES: usize = 256 * 1024;
/// The engine's window for a thumbnail, and the thumbnail.
const SHOT_W: u32 = 1280;
const SHOT_H: u32 = 800;
const THUMB_W: u32 = 640;
const THUMB_WAIT: Duration = Duration::from_secs(25);
/// After the page says it has loaded: time for late images and fonts.
const SETTLE: Duration = Duration::from_millis(1800);

fn http() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| {
        reqwest::Client::builder()
            // Whether the site answers is checked apart from its certificate (checked below).
            .danger_accept_invalid_certs(true)
            .redirect(reqwest::redirect::Policy::limited(10))
            .connect_timeout(Duration::from_secs(10))
            .timeout(TIMEOUT)
            .user_agent(concat!("Mozilla/5.0 (compatible; RAD-Web-Monitor/", env!("CARGO_PKG_VERSION"), ")"))
            .build()
            .expect("http client")
    })
}

fn parse_url(url: &str) -> CmdResult<reqwest::Url> {
    let u = url.trim();
    let u = if u.contains("://") { u.to_string() } else { format!("https://{u}") };
    let u = reqwest::Url::parse(&u).map_err(|_| CmdError::new("bad_request", "That isn't a web address"))?;
    if !matches!(u.scheme(), "http" | "https") || u.host_str().is_none() {
        return Err(CmdError::new("bad_request", "Use an http:// or https:// address"));
    }
    Ok(u)
}

// ------------------------------------------------------------------ certificates

/// One DER element: its tag, its contents and what follows it.
fn der(b: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, b) = b.split_first()?;
    let (&l0, mut b) = b.split_first()?;
    let len = if l0 < 0x80 {
        l0 as usize
    } else {
        let n = (l0 & 0x7f) as usize;
        if n == 0 || n > 4 || b.len() < n {
            return None;
        }
        let l = b[..n].iter().fold(0usize, |l, &x| l << 8 | x as usize);
        b = &b[n..];
        l
    };
    (b.len() >= len).then(|| (tag, &b[..len], &b[len..]))
}

/// Days since 1970-01-01 of a calendar date (proleptic Gregorian).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// UTCTime (YYMMDDHHMMSSZ) or GeneralizedTime (YYYYMMDDHHMMSSZ), in Unix milliseconds.
fn der_time(tag: u8, b: &[u8]) -> Option<i64> {
    let s = std::str::from_utf8(b).ok()?.trim_end_matches('Z');
    let (y, rest) = match tag {
        0x17 => {
            let yy: i64 = s.get(..2)?.parse().ok()?;
            (if yy < 50 { 2000 + yy } else { 1900 + yy }, s.get(2..)?)
        }
        0x18 => (s.get(..4)?.parse().ok()?, s.get(4..)?),
        _ => return None,
    };
    let n = |i: usize| -> Option<i64> { rest.get(i..i + 2)?.parse().ok() };
    let secs = days_from_civil(y, n(0)?, n(2)?) * 86_400 + n(4)? * 3600 + n(6)? * 60 + n(8).unwrap_or(0);
    Some(secs * 1000)
}

/// The value of an attribute (an OID's last byte under 2.5.4) in an X.509 name.
fn name_part(name: &[u8], attr: u8) -> Option<String> {
    let mut sets = name;
    while let Some((_, set, rest)) = der(sets) {
        sets = rest;
        let Some((_, atv, _)) = der(set) else { continue };
        let Some((_, oid, val)) = der(atv) else { continue };
        if oid == [0x55, 0x04, attr] {
            return der(val).map(|(_, v, _)| String::from_utf8_lossy(v).into_owned());
        }
    }
    None
}

#[derive(Debug, PartialEq)]
struct CertInfo {
    not_before: i64,
    not_after: i64,
    issuer: String,
    subject: String,
    names: Vec<String>,
}

/// What a certificate says about itself (only the fields shown; nothing is verified here).
fn parse_cert(cert: &[u8]) -> Option<CertInfo> {
    let (_, c, _) = der(cert)?;
    let (_, tbs, _) = der(c)?;
    let mut rest = tbs;
    let (tag, _, r) = der(rest)?;
    if tag == 0xa0 {
        rest = r; // version
    }
    let (_, _, r) = der(rest)?; // serial number
    let (_, _, r) = der(r)?; // signature algorithm
    let (_, issuer, r) = der(r)?;
    let (_, validity, r) = der(r)?;
    let (_, subject, r) = der(r)?;
    let (t1, nb, v) = der(validity)?;
    let (t2, na, _) = der(v)?;
    // Extensions ([3]) after the public key: the names it covers (subjectAltName dNSNames).
    let mut names = Vec::new();
    let (_, _, mut r) = der(r)?;
    while let Some((tag, body, next)) = der(r) {
        r = next;
        if tag != 0xa3 {
            continue;
        }
        let Some((_, mut exts, _)) = der(body) else { break };
        while let Some((_, ext, more)) = der(exts) {
            exts = more;
            let Some((_, oid, mut v)) = der(ext) else { continue };
            if oid != [0x55, 0x1d, 0x11] {
                continue;
            }
            if let Some((0x01, _, after)) = der(v) {
                v = after; // critical
            }
            let Some((_, octets, _)) = der(v) else { continue };
            let Some((_, mut gns, _)) = der(octets) else { continue };
            while let Some((t, n, more)) = der(gns) {
                gns = more;
                if t == 0x82 && names.len() < 50 {
                    names.push(String::from_utf8_lossy(n).into_owned());
                }
            }
        }
    }
    Some(CertInfo {
        not_before: der_time(t1, nb)?,
        not_after: der_time(t2, na)?,
        // "Let's Encrypt (R11)": the organisation, and its certificate's name.
        issuer: match (name_part(issuer, 0x0a), name_part(issuer, 0x03)) {
            (Some(o), Some(cn)) if o != cn => format!("{o} ({cn})"),
            (o, cn) => o.or(cn).unwrap_or_default(),
        },
        subject: name_part(subject, 0x03).unwrap_or_default(),
        names,
    })
}

/// Verifies like a browser would, but lets the handshake finish either way, so an expired or
/// wrong certificate can still be read and reported.
#[derive(Debug)]
struct Recording {
    inner: Arc<rustls::client::WebPkiServerVerifier>,
    verdict: Mutex<Option<Result<(), String>>>,
}

impl rustls::client::danger::ServerCertVerifier for Recording {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        intermediates: &[rustls::pki_types::CertificateDer<'_>],
        server_name: &rustls::pki_types::ServerName<'_>,
        ocsp: &[u8],
        now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        let r = self.inner.verify_server_cert(end_entity, intermediates, server_name, ocsp, now);
        *self.verdict.lock().unwrap() = Some(r.map(|_| ()).map_err(|e| e.to_string()));
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(&self, m: &[u8], c: &rustls::pki_types::CertificateDer<'_>, d: &rustls::DigitallySignedStruct) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(m, c, d)
    }
    fn verify_tls13_signature(&self, m: &[u8], c: &rustls::pki_types::CertificateDer<'_>, d: &rustls::DigitallySignedStruct) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(m, c, d)
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

/// rustls' reason, in words for people.
fn cert_problem(e: &str) -> String {
    let l = e.to_ascii_lowercase();
    let has = |a: &str, b: &str| l.contains(a) || l.contains(b);
    if has("expired", "notvalidafter") {
        "the certificate has expired".into()
    } else if has("not valid for name", "notvalidforname") {
        "the certificate isn't for this name".into()
    } else if has("unknown issuer", "unknownissuer") {
        "the certificate isn't from a trusted issuer (self-signed?)".into()
    } else if has("not valid yet", "notvalidyet") {
        "the certificate isn't valid yet".into()
    } else if l.contains("revoked") {
        "the certificate was revoked".into()
    } else {
        e.trim_start_matches("invalid peer certificate: ").to_string()
    }
}

async fn certificate(host: &str, port: u16) -> Result<Value, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let inner = rustls::client::WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider.clone()).build().map_err(|e| e.to_string())?;
    let verifier = Arc::new(Recording { inner, verdict: Mutex::new(None) });
    let cfg = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(verifier.clone())
        .with_no_client_auth();
    let name = rustls::pki_types::ServerName::try_from(host.to_string()).map_err(|_| "that host name isn't valid".to_string())?;
    let tcp = tokio::time::timeout(Duration::from_secs(10), tokio::net::TcpStream::connect((host, port)))
        .await
        .map_err(|_| "no answer on the HTTPS port".to_string())?
        .map_err(|e| format!("can't connect: {e}"))?;
    let tls = tokio::time::timeout(Duration::from_secs(10), tokio_rustls::TlsConnector::from(Arc::new(cfg)).connect(name, tcp))
        .await
        .map_err(|_| "the TLS handshake timed out".to_string())?
        .map_err(|e| format!("TLS handshake failed: {e}"))?;
    let (_, conn) = tls.get_ref();
    let cert = conn.peer_certificates().and_then(|c| c.first()).ok_or("the server sent no certificate")?;
    let info = parse_cert(cert.as_ref()).ok_or("the certificate couldn't be read")?;
    let verdict = verifier.verdict.lock().unwrap().clone().unwrap_or(Ok(()));
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
    Ok(json!({
        "valid": verdict.is_ok(),
        "problem": verdict.err().map(|e| cert_problem(&e)),
        "issuer": info.issuer,
        "subject": info.subject,
        "names": info.names,
        "not_before": info.not_before,
        "expires": info.not_after,
        "days_left": (info.not_after - now).div_euclid(86_400_000),
        "protocol": conn.protocol_version().map(|v| format!("{v:?}").replace('_', ".").replace("TLSv", "TLS ")),
    }))
}

/// The page's <title>, from the start of its HTML.
fn title_of(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let open = start + lower[start..].find('>')? + 1;
    let end = open + lower[open..].find("</title")?;
    let t = html[open..end].split_whitespace().collect::<Vec<_>>().join(" ");
    let t = t.replace("&amp;", "&").replace("&#39;", "'").replace("&quot;", "\"").replace("&lt;", "<").replace("&gt;", ">");
    (!t.is_empty()).then(|| t.chars().take(200).collect())
}

/// Checks a website now: `{online, status, ms, final_url, title, ips, certificate, error}`.
#[tauri::command]
pub async fn monitor_check(url: String) -> CmdResult<Value> {
    let u = parse_url(&url)?;
    let host = u.host_str().unwrap_or_default().trim_matches(['[', ']']).to_string();
    let port = u.port_or_known_default().unwrap_or(443);
    let checked_at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);

    let dns = async {
        match tokio::time::timeout(Duration::from_secs(8), tokio::net::lookup_host((host.as_str(), port))).await {
            Ok(Ok(addrs)) => {
                let mut ips: Vec<std::net::IpAddr> = addrs.map(|a| a.ip()).collect();
                ips.sort_by_key(|ip| (ip.is_ipv6(), *ip));
                ips.dedup();
                Ok(ips.into_iter().map(|ip| ip.to_string()).collect::<Vec<_>>())
            }
            Ok(Err(e)) => Err(format!("the name doesn't resolve: {e}")),
            Err(_) => Err("the name lookup timed out".to_string()),
        }
    };
    let page = async {
        let t = Instant::now();
        let mut r = http().get(u.clone()).send().await.map_err(|e| {
            let mut s = e.to_string();
            let mut src = std::error::Error::source(&e);
            while let Some(c) = src {
                s = c.to_string();
                src = c.source();
            }
            if e.is_timeout() { "no answer in 20 seconds".to_string() } else { s }
        })?;
        let ms = t.elapsed().as_millis() as u64;
        let status = r.status().as_u16();
        let final_url = r.url().to_string();
        let mut body = Vec::new();
        while body.len() < TITLE_BYTES {
            match tokio::time::timeout(Duration::from_secs(5), r.chunk()).await {
                Ok(Ok(Some(c))) => body.extend_from_slice(&c),
                _ => break,
            }
        }
        Ok::<_, String>((status, ms, final_url, title_of(&String::from_utf8_lossy(&body))))
    };
    let cert = async {
        if u.scheme() == "https" {
            Some(certificate(&host, port).await)
        } else {
            None
        }
    };
    let (ips, page, cert) = tokio::join!(dns, page, cert);
    let mut errors = Vec::new();
    let ips = ips.unwrap_or_else(|e| {
        errors.push(e);
        Vec::new()
    });
    let (online, status, ms, final_url, title) = match page {
        Ok((status, ms, final_url, title)) => (status < 500, Some(status), Some(ms), Some(final_url), title),
        Err(e) => {
            errors.push(e);
            (false, None, None, None, None)
        }
    };
    if let Some(s) = status.filter(|s| *s >= 500) {
        errors.push(format!("the server answered with an error (HTTP {s})"));
    }
    let certificate = match cert {
        Some(Ok(c)) => c,
        Some(Err(e)) => json!({"error": e}),
        None => Value::Null,
    };
    Ok(json!({
        "url": u.to_string(),
        "host": host,
        "checked_at": checked_at,
        "online": online,
        "status": status,
        "ms": ms,
        "final_url": final_url,
        "title": title,
        "ips": ips,
        "certificate": certificate,
        "error": (!errors.is_empty()).then(|| errors.join("; ")),
    }))
}

// ------------------------------------------------------------------ thumbnails

/// Takes a picture of a website: its first screen, scaled to 640 pixels wide (JPEG).
#[tauri::command]
pub async fn monitor_thumb(app: AppHandle, url: String) -> CmdResult<Response> {
    // One at a time: each is a browser engine of its own.
    static ONE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _one = ONE.lock().await;
    let u = parse_url(&url)?.to_string();
    let (bin, chromium) = crate::browser::snapshot_engine(&app).ok_or_else(|| CmdError::new("browser_missing", "No browser engine is built to take thumbnails (run ./scripts/build-chromium.sh or ./scripts/build-browser.sh)"))?;
    let profile = app.path().app_data_dir().map(|d| d.join("monitor-profile")).map_err(|e| CmdError::new("monitor", e.to_string()))?;
    let jpeg = tokio::task::spawn_blocking(move || shoot(bin, chromium, profile, &u)).await.map_err(|e| CmdError::new("monitor", e.to_string()))??;
    Ok(Response::new(jpeg))
}

enum Msg {
    Loaded(bool),
    Frame,
}

fn shoot(bin: PathBuf, chromium: bool, profile: PathBuf, url: &str) -> CmdResult<Vec<u8>> {
    use std::io::Read;
    let mut cmd = Command::new(&bin);
    cmd.args(["--width", &SHOT_W.to_string(), "--height", &SHOT_H.to_string(), "--scale", "1", "--url", url]);
    if chromium {
        let _ = std::fs::create_dir_all(&profile);
        cmd.arg("--profile").arg(&profile).arg("--downloads").arg(profile.join("downloads"));
        if cfg!(target_os = "linux") {
            if let Some(dir) = bin.parent() {
                cmd.env("LD_LIBRARY_PATH", dir);
            }
        }
    }
    let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|e| CmdError::new("monitor", format!("Couldn't start the browser engine: {e}")))?;
    let stdout = child.stdout.take().expect("piped stdout");
    let frame: Arc<Mutex<(u32, u32, Vec<u8>)>> = Arc::default();
    let (tx, rx) = mpsc::channel::<Msg>();
    let f2 = frame.clone();
    // The engine's output: JSON events (1) and changed rows of the picture (2), each framed as
    // a kind byte and a little-endian length.
    std::thread::spawn(move || {
        let mut r = std::io::BufReader::with_capacity(1 << 20, stdout);
        let mut head = [0u8; 5];
        let mut buf = Vec::new();
        while r.read_exact(&mut head).is_ok() {
            buf.resize(u32::from_le_bytes([head[1], head[2], head[3], head[4]]) as usize, 0);
            if r.read_exact(&mut buf).is_err() {
                break;
            }
            match head[0] {
                1 => {
                    let Ok(v) = serde_json::from_slice::<Value>(&buf) else { continue };
                    if v["type"] == "state" {
                        let active = &v["active"];
                        let tab = v["tabs"].as_array().and_then(|t| t.iter().find(|t| &t["id"] == active).or(t.first()));
                        if let Some(t) = tab {
                            let real = t["url"].as_str().is_some_and(|u| u.starts_with("http"));
                            let _ = tx.send(Msg::Loaded(real && t["loading"] == false));
                        }
                    }
                }
                2 if buf.len() >= 16 => {
                    let n = |i: usize| u32::from_le_bytes([buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]);
                    let (w, h, y0, rows) = (n(0), n(4), n(8), n(12));
                    let stride = w as usize * 4;
                    if y0 + rows > h || buf.len() < 16 + rows as usize * stride {
                        continue;
                    }
                    let mut f = f2.lock().unwrap();
                    if f.0 != w || f.1 != h {
                        *f = (w, h, vec![0; stride * h as usize]);
                    }
                    let start = y0 as usize * stride;
                    f.2[start..start + rows as usize * stride].copy_from_slice(&buf[16..16 + rows as usize * stride]);
                    drop(f);
                    let _ = tx.send(Msg::Frame);
                }
                _ => {}
            }
        }
    });
    let deadline = Instant::now() + THUMB_WAIT;
    let mut loaded_at: Option<Instant> = None;
    let mut frames = 0u32;
    while Instant::now() < deadline {
        if loaded_at.is_some_and(|t| t.elapsed() >= SETTLE) && frames > 0 {
            break;
        }
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(Msg::Loaded(true)) => {
                loaded_at.get_or_insert_with(Instant::now);
            }
            Ok(Msg::Loaded(false)) => loaded_at = None,
            Ok(Msg::Frame) => frames += 1,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    let (w, h, px) = std::mem::take(&mut *frame.lock().unwrap());
    if frames == 0 || px.iter().all(|&b| b == 0) {
        return Err(CmdError::new("monitor", "The page didn't show anything in time"));
    }
    let img = image::RgbaImage::from_raw(w, h, px).ok_or_else(|| CmdError::new("monitor", "bad frame"))?;
    let th = (THUMB_W as u64 * h as u64 / w.max(1) as u64) as u32;
    let small = image::imageops::resize(&img, THUMB_W, th.max(1), image::imageops::FilterType::Triangle);
    let rgb = image::DynamicImage::ImageRgba8(small).to_rgb8();
    let mut out = std::io::Cursor::new(Vec::new());
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 78).encode_image(&rgb).map_err(|e| CmdError::new("monitor", e.to_string()))?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use base64::Engine;

    use super::*;

    /// openssl req -x509 … -subj "/O=RAD Test CA/CN=booksia.test" -not_before 20260101000000Z -not_after 20270315120000Z
    const CERT: &str = "MIIB2jCCAYCgAwIBAgIUDqxvQvdEgDWG/+5T94Cybhw6VBQwCgYIKoZIzj0EAwIwLTEUMBIGA1UECgwLUkFEIFRlc3QgQ0ExFTATBgNVBAMMDGJvb2tzaWEudGVzdDAeFw0yNjAxMDEwMDAwMDBaFw0yNzAzMTUxMjAwMDBaMC0xFDASBgNVBAoMC1JBRCBUZXN0IENBMRUwEwYDVQQDDAxib29rc2lhLnRlc3QwWTATBgcqhkjOPQIBBggqhkjOPQMBBwNCAAQd+cHIrGcVAvqu/7YV/+vas+Donh/GUhBRiOrF3QLwEBVma7qEHeiFoZT13fJL4fBCCL+VmulA6J+PY/qlJr8co34wfDAdBgNVHQ4EFgQUjsyI0GkYF509VTEyIylVXz30QVAwHwYDVR0jBBgwFoAUjsyI0GkYF509VTEyIylVXz30QVAwDwYDVR0TAQH/BAUwAwEB/zApBgNVHREEIjAgggxib29rc2lhLnRlc3SCEHd3dy5ib29rc2lhLnRlc3QwCgYIKoZIzj0EAwIDSAAwRQIgHX4TVrBAPWnIkUbx14C+tJOq/Cwrh9ZYAuqCJCo30DsCIQCa8kd/UcSu3xRXzTEClqlp83M5RBRzXQmFhktcLB2Bdg==";

    #[test]
    fn reads_a_certificate() {
        let der = base64::engine::general_purpose::STANDARD.decode(CERT).unwrap();
        let c = parse_cert(&der).unwrap();
        assert_eq!(c.not_before, 1_767_225_600_000);
        assert_eq!(c.not_after, 1_805_112_000_000);
        assert_eq!(c.subject, "booksia.test");
        assert_eq!(c.issuer, "RAD Test CA (booksia.test)");
        assert_eq!(c.names, ["booksia.test", "www.booksia.test"]);
        assert!(parse_cert(&der[..100]).is_none());
    }

    #[test]
    fn times_and_titles() {
        assert_eq!(der_time(0x17, b"700101000000Z"), Some(0));
        assert_eq!(der_time(0x18, b"20000301000000Z"), Some(951_868_800_000));
        assert_eq!(title_of("<html><head><TITLE>\n Booksia &amp; Co </title>"), Some("Booksia & Co".into()));
        assert_eq!(title_of("<p>none</p>"), None);
        assert_eq!(parse_url("booksia.com.au").unwrap().as_str(), "https://booksia.com.au/");
        assert!(parse_url("ftp://x").is_err());
    }

    /// Needs the network (and, for the thumbnail, a built engine): cargo test --lib monitor -- --ignored
    #[tokio::test]
    #[ignore]
    async fn live() {
        let ok = monitor_check("https://badssl.com".into()).await.unwrap();
        assert_eq!(ok["online"], true, "{ok:#}");
        assert_eq!(ok["certificate"]["valid"], true, "{ok:#}");
        assert!(ok["certificate"]["days_left"].as_i64().unwrap() > 0);
        assert!(!ok["ips"].as_array().unwrap().is_empty());
        println!("{:#}", monitor_check("booksia.com.au".into()).await.unwrap());
        let bad = monitor_check("https://expired.badssl.com".into()).await.unwrap();
        assert_eq!(bad["certificate"]["valid"], false, "{bad:#}");
        assert_eq!(bad["certificate"]["problem"], "the certificate has expired", "{bad:#}");
        assert!(bad["certificate"]["days_left"].as_i64().unwrap() < 0);
        let wrong = monitor_check("https://wrong.host.badssl.com".into()).await.unwrap();
        assert_eq!(wrong["certificate"]["problem"], "the certificate isn't for this name", "{wrong:#}");
        let gone = monitor_check("https://no-such-site.invalid".into()).await.unwrap();
        assert_eq!(gone["online"], false);
        println!("{}", gone["error"]);

        let bin = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../browser/rad-chromium/target/bundle/rad-chromium.app/Contents/MacOS/rad-chromium");
        if bin.is_file() {
            let dir = std::env::temp_dir().join("rad-monitor-test-profile");
            let jpeg = tokio::task::spawn_blocking(move || shoot(bin, true, dir, "https://booksia.com.au")).await.unwrap().unwrap();
            let out = std::env::temp_dir().join("rad-monitor-thumb.jpg");
            std::fs::write(&out, &jpeg).unwrap();
            println!("thumbnail: {} ({} bytes)", out.display(), jpeg.len());
        }
    }
}
