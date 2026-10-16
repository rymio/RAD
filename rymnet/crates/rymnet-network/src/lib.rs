use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rymnet_core::{NodeIdentity, sign_json_value, verify_json_value};
use rymnet_storage::PeerEntry;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use snow::TransportState;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, info, warn};
use uuid::Uuid;

const NOISE_PATTERN: &str = "Noise_NN_25519_ChaChaPoly_BLAKE2s";
const MAX_FRAME: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PeerMessage {
    #[serde(rename = "type")]
    pub message_type: String,
    pub version: u16,
    pub request_id: String,
    pub payload: Value,
}

impl PeerMessage {
    pub fn new(message_type: impl Into<String>, payload: Value) -> Self {
        Self {
            message_type: message_type.into(),
            version: 1,
            request_id: Uuid::new_v4().to_string(),
            payload,
        }
    }

    pub fn response_to(
        request: &PeerMessage,
        message_type: impl Into<String>,
        payload: Value,
    ) -> Self {
        Self {
            message_type: message_type.into(),
            version: 1,
            request_id: request.request_id.clone(),
            payload,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PeerHello {
    #[serde(rename = "type")]
    pub record_type: String,
    pub version: u16,
    pub node_id: String,
    pub public_key: String,
    pub signature: String,
}

impl PeerHello {
    pub fn create(identity: &NodeIdentity) -> Result<Self> {
        let unsigned = hello_unsigned(identity)?;
        let signature = sign_json_value(&identity.signing_key()?, &unsigned)?;
        Ok(Self {
            record_type: "rymnet.peer.hello".to_string(),
            version: 1,
            node_id: identity.node_id()?.to_string(),
            public_key: identity.public_key.clone(),
            signature,
        })
    }

    pub fn verify(&self) -> Result<bool> {
        verify_json_value(&self.public_key, &self.signature, &self.unsigned_value())
    }

    fn unsigned_value(&self) -> Value {
        json!({
            "type": "rymnet.peer.hello",
            "version": 1,
            "node_id": self.node_id,
            "public_key": self.public_key,
        })
    }
}

#[derive(Clone, Debug)]
pub struct PeerSessionInfo {
    pub hello: PeerHello,
    pub remote_addr: String,
}

pub type Handler = Arc<
    dyn Fn(
            PeerSessionInfo,
            PeerMessage,
        ) -> Pin<Box<dyn Future<Output = Result<PeerMessage>> + Send>>
        + Send
        + Sync,
>;

/// Opens a TCP connection to a peer. When `RYMNET_SOCKS` is set (`127.0.0.1:port`, as RAD
/// does while its VPN covers Rymnet), the connection goes through that SOCKS5 proxy; the peer
/// protocol's own Noise encryption runs inside it either way.
async fn connect_endpoint(endpoint: &str) -> Result<TcpStream> {
    match std::env::var("RYMNET_SOCKS").ok().filter(|s| !s.trim().is_empty()) {
        Some(proxy) => Ok(tokio_socks::tcp::Socks5Stream::connect(proxy.trim(), endpoint)
            .await
            .with_context(|| format!("through the VPN proxy {proxy}"))?
            .into_inner()),
        None => Ok(TcpStream::connect(endpoint).await?),
    }
}

pub async fn send_request(
    peer: &PeerEntry,
    identity: &NodeIdentity,
    request: PeerMessage,
) -> Result<PeerMessage> {
    let stream = connect_endpoint(&peer.endpoint)
        .await
        .with_context(|| format!("connect peer {} at {}", peer.name, peer.endpoint))?;
    let remote_addr = stream
        .peer_addr()
        .map(|addr| addr.to_string())
        .unwrap_or_else(|_| peer.endpoint.clone());
    let mut session = handshake_initiator(stream).await?;
    let remote_hello = exchange_hello_initiator(&mut session, identity).await?;
    validate_expected_peer(peer, &remote_hello)?;
    debug!("connected encrypted peer session to {}", remote_addr);
    write_message(&mut session, &request).await?;
    read_message(&mut session).await
}

pub async fn ping(peer: &PeerEntry, identity: &NodeIdentity) -> Result<PeerMessage> {
    send_request(peer, identity, PeerMessage::new("peer.status", json!({}))).await
}

/// Milliseconds since the Unix epoch, per this node's own system clock.
/// Rymnet has no central time authority, so this -- each node's own wall
/// clock -- is the one reference every offset in `time_sync` is measured
/// against.
pub fn unix_millis_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

/// One peer's result from `time_sync`: an NTP-style clock offset (peer
/// minus us) estimated from a single request/response round trip, on the
/// simplifying assumption that the trip is symmetric -- half the RTT
/// elapses on the way out, half on the way back.
#[derive(Clone, Copy, Debug)]
pub struct TimeSample {
    pub peer_time_ms: i64,
    pub round_trip_ms: i64,
    pub offset_ms: i64,
}

pub async fn time_sync(peer: &PeerEntry, identity: &NodeIdentity) -> Result<TimeSample> {
    let sent_at = unix_millis_now();
    let response = send_request(
        peer,
        identity,
        PeerMessage::new("time.sync", json!({"client_time_ms": sent_at})),
    )
    .await?;
    let received_at = unix_millis_now();
    let peer_time_ms = response
        .payload
        .get("server_time_ms")
        .and_then(Value::as_i64)
        .context("time.sync response missing server_time_ms")?;
    let round_trip_ms = received_at - sent_at;
    let offset_ms = peer_time_ms - (sent_at + round_trip_ms / 2);
    Ok(TimeSample { peer_time_ms, round_trip_ms, offset_ms })
}

/// First contact with a node whose public key isn't known yet: connects
/// and completes the handshake without pinning against an expected key
/// (there is nothing to pin against yet), then sends one request. The
/// returned `PeerHello` is how the caller learns the remote's real
/// identity -- it is still signed and verified like any other hello, so
/// the *key* is trustworthy even though it wasn't known in advance; it's
/// the caller's job to decide whether the *node* is trustworthy before
/// acting on anything further.
pub async fn send_request_discover(
    endpoint: &str,
    identity: &NodeIdentity,
    request: PeerMessage,
) -> Result<(PeerHello, PeerMessage)> {
    let stream = connect_endpoint(endpoint)
        .await
        .with_context(|| format!("connect {endpoint}"))?;
    let mut session = handshake_initiator(stream).await?;
    let remote_hello = exchange_hello_initiator(&mut session, identity).await?;
    write_message(&mut session, &request).await?;
    let response = read_message(&mut session).await?;
    Ok((remote_hello, response))
}

pub async fn serve(listen_addr: &str, identity: NodeIdentity, handler: Handler) -> Result<()> {
    let listener = TcpListener::bind(listen_addr)
        .await
        .with_context(|| format!("bind peer listener {listen_addr}"))?;
    info!("peer listener bound on {listen_addr}");

    loop {
        let (stream, addr) = listener.accept().await.context("accept peer")?;
        let identity = identity.clone();
        let handler = handler.clone();
        tokio::spawn(async move {
            if let Err(err) = handle_connection(stream, addr.to_string(), identity, handler).await {
                warn!("peer connection failed: {err:#}");
            }
        });
    }
}

async fn handle_connection(
    stream: TcpStream,
    remote_addr: String,
    identity: NodeIdentity,
    handler: Handler,
) -> Result<()> {
    let mut session = handshake_responder(stream).await?;
    let hello = exchange_hello_responder(&mut session, &identity).await?;
    if !hello.verify()? {
        anyhow::bail!("peer hello signature failed");
    }
    let request = read_message(&mut session).await?;
    let info = PeerSessionInfo { hello, remote_addr };
    let response = handler(info, request).await?;
    write_message(&mut session, &response).await
}

async fn handshake_initiator(mut stream: TcpStream) -> Result<EncryptedStream> {
    let params = NOISE_PATTERN.parse()?;
    let builder = snow::Builder::new(params);
    let mut noise = builder.build_initiator()?;
    let mut buf = [0u8; 65535];

    let len = noise.write_message(&[], &mut buf)?;
    write_plain_frame(&mut stream, &buf[..len]).await?;
    let msg = read_plain_frame(&mut stream).await?;
    noise.read_message(&msg, &mut buf)?;

    Ok(EncryptedStream {
        stream,
        noise: noise.into_transport_mode()?,
    })
}

async fn handshake_responder(mut stream: TcpStream) -> Result<EncryptedStream> {
    let params = NOISE_PATTERN.parse()?;
    let builder = snow::Builder::new(params);
    let mut noise = builder.build_responder()?;
    let mut buf = [0u8; 65535];

    let msg = read_plain_frame(&mut stream).await?;
    noise.read_message(&msg, &mut buf)?;
    let len = noise.write_message(&[], &mut buf)?;
    write_plain_frame(&mut stream, &buf[..len]).await?;

    Ok(EncryptedStream {
        stream,
        noise: noise.into_transport_mode()?,
    })
}

async fn exchange_hello_initiator(
    session: &mut EncryptedStream,
    identity: &NodeIdentity,
) -> Result<PeerHello> {
    write_encrypted_frame(session, &serde_json::to_vec(&PeerHello::create(identity)?)?).await?;
    let bytes = read_encrypted_frame(session).await?;
    let hello: PeerHello = serde_json::from_slice(&bytes).context("parse peer hello")?;
    if !hello.verify()? {
        anyhow::bail!("peer hello signature failed");
    }
    Ok(hello)
}

async fn exchange_hello_responder(
    session: &mut EncryptedStream,
    identity: &NodeIdentity,
) -> Result<PeerHello> {
    let bytes = read_encrypted_frame(session).await?;
    let hello: PeerHello = serde_json::from_slice(&bytes).context("parse peer hello")?;
    write_encrypted_frame(session, &serde_json::to_vec(&PeerHello::create(identity)?)?).await?;
    Ok(hello)
}

async fn write_message(session: &mut EncryptedStream, message: &PeerMessage) -> Result<()> {
    write_encrypted_frame(session, &serde_json::to_vec(message)?).await
}

async fn read_message(session: &mut EncryptedStream) -> Result<PeerMessage> {
    let bytes = read_encrypted_frame(session).await?;
    serde_json::from_slice(&bytes).context("parse peer message")
}

struct EncryptedStream {
    stream: TcpStream,
    noise: TransportState,
}

/// The Noise Protocol caps a single transport message at 65535 bytes
/// (including its AEAD tag). Messages larger than that -- e.g. a chunk
/// offer carrying a base64'd multi-megabyte file -- are split into several
/// Noise messages here and reassembled on the other end, each still framed
/// individually by `write_plain_frame`/`read_plain_frame`. A 4-byte total
/// length prefix precedes the first chunk so the reader knows when it has
/// the whole thing.
const MAX_NOISE_CHUNK: usize = 60_000;

async fn write_encrypted_frame(session: &mut EncryptedStream, plaintext: &[u8]) -> Result<()> {
    let mut framed = Vec::with_capacity(plaintext.len() + 4);
    framed.extend_from_slice(&(plaintext.len() as u32).to_be_bytes());
    framed.extend_from_slice(plaintext);

    for chunk in framed.chunks(MAX_NOISE_CHUNK) {
        let mut out = vec![0u8; chunk.len() + 1024];
        let len = session.noise.write_message(chunk, &mut out)?;
        write_plain_frame(&mut session.stream, &out[..len]).await?;
    }
    Ok(())
}

async fn read_encrypted_frame(session: &mut EncryptedStream) -> Result<Vec<u8>> {
    let mut buffer = Vec::new();
    let mut expected_len: Option<usize> = None;

    loop {
        let ciphertext = read_plain_frame(&mut session.stream).await?;
        let mut out = vec![0u8; ciphertext.len() + 1024];
        let len = session.noise.read_message(&ciphertext, &mut out)?;
        buffer.extend_from_slice(&out[..len]);

        if expected_len.is_none() && buffer.len() >= 4 {
            let total = u32::from_be_bytes(buffer[0..4].try_into().unwrap()) as usize;
            expected_len = Some(total);
        }

        if let Some(total) = expected_len
            && buffer.len() >= total + 4
        {
            buffer.drain(0..4);
            buffer.truncate(total);
            return Ok(buffer);
        }
    }
}

async fn write_plain_frame(stream: &mut TcpStream, bytes: &[u8]) -> Result<()> {
    if bytes.len() > MAX_FRAME {
        anyhow::bail!("frame too large");
    }
    stream.write_u32(bytes.len() as u32).await?;
    stream.write_all(bytes).await?;
    Ok(())
}

async fn read_plain_frame(stream: &mut TcpStream) -> Result<Vec<u8>> {
    let len = stream.read_u32().await? as usize;
    if len > MAX_FRAME {
        anyhow::bail!("frame too large");
    }
    let mut bytes = vec![0u8; len];
    stream.read_exact(&mut bytes).await?;
    Ok(bytes)
}

fn validate_expected_peer(peer: &PeerEntry, hello: &PeerHello) -> Result<()> {
    if peer.public_key != "*" && peer.public_key != hello.public_key {
        anyhow::bail!("peer public key mismatch for {}", peer.name);
    }
    Ok(())
}

fn hello_unsigned(identity: &NodeIdentity) -> Result<Value> {
    Ok(json!({
        "type": "rymnet.peer.hello",
        "version": 1,
        "node_id": identity.node_id()?.to_string(),
        "public_key": identity.public_key,
    }))
}

pub fn bytes_to_b64(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

pub fn b64_to_bytes(value: &str) -> Result<Vec<u8>> {
    STANDARD.decode(value).context("decode base64")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_message_protocol() {
        let message = PeerMessage::new("peer.status", json!({"ok": true}));
        let json = serde_json::to_value(&message).expect("json");
        assert_eq!(json["type"], "peer.status");
        assert_eq!(json["version"], 1);
        assert!(json["request_id"].as_str().unwrap().len() > 8);
    }

    #[test]
    fn signed_hello_verifies() {
        let identity = NodeIdentity::generate();
        let hello = PeerHello::create(&identity).expect("hello");
        assert!(hello.verify().expect("verify"));
    }
}
