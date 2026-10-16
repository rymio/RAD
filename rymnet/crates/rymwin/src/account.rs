use anyhow::{Context, Result};
use rymnet_account::{
    CIPHER_KEY_LEN, PlainVfsManifest, VfsFileEntry, VfsPointerRecord, check_quota,
    claim_username, decrypt, derive_account, encrypt, find_claim, find_pointer,
    merge_remote_claim, validate_username,
};
use rymnet_core::NodeIdentity;
use rymnet_mail::{MailboxKeyRecord, derive_mailbox_keypair};
use rymnet_network::{PeerMessage, send_request};
use rymnet_storage::{AppConfig, ChunkStore, DataLayout, PeerConfig, PeerEntry};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;
use x25519_dalek::StaticSecret;

pub struct LoggedInAccount {
    pub username: String,
    pub identity: NodeIdentity,
    pub cipher_key: [u8; CIPHER_KEY_LEN],
    // Unused until a mail app exists, but it can only be re-derived from
    // the plaintext password, which login() deliberately doesn't retain --
    // dropping this now would mean re-prompting for the password later.
    #[allow(dead_code)]
    pub mailbox_secret: StaticSecret,
    pub vfs_version: u64,
    pub cache_dir: PathBuf,
    pub peers_reached: usize,
    pub fresh_account: bool,
    pub files_restored: usize,
}

/// Deliberately re-derives everything from the password locally (the
/// "cheapest MVP" this was scoped as) rather than reusing a session from
/// any other rymnet process -- there is no key handoff across process
/// boundaries here, on purpose.
pub async fn login(layout: &DataLayout, username: &str, password: &str) -> Result<LoggedInAccount> {
    validate_username(username)?;
    let derived = derive_account(username, password)?;
    let identity = derived.identity;
    let cipher_key = derived.cipher_key;

    claim_username(layout.account_claims_dir(), username, &identity)
        .context("this username is already registered locally under a different password")?;

    let peers = PeerConfig::load(layout.peers_path())?;
    let local_claim =
        find_claim(layout.account_claims_dir(), username)?.expect("just saved by claim_username");

    let mut peers_reached = 0usize;
    for peer in &peers.peers {
        if announce_claim(peer, &identity, &local_claim, layout).await {
            peers_reached += 1;
        }
    }

    let confirmed_claim = find_claim(layout.account_claims_dir(), username)?
        .expect("claim still present after announce");
    if confirmed_claim.owner_public_key != identity.public_key {
        anyhow::bail!(
            "username '{username}' is already registered under a different password; \
             choose a different username, or log in with the correct password"
        );
    }

    let cache_dir = cache_dir_for_user(layout, username);
    let (vfs_version, files_restored, fresh_account) =
        pull_vfs(layout, &peers, &identity, &cipher_key, username, &cache_dir).await?;
    if fresh_account {
        // Best-effort: a missing example program isn't worth failing
        // login over.
        let _ = create_starter_programs(&cache_dir);
    }

    let mailbox_keys = derive_mailbox_keypair(username, password)?;
    let mailbox_key_record = MailboxKeyRecord::create(username, &identity, &mailbox_keys.public)?;
    mailbox_key_record.save(layout.mailbox_keys_dir())?;
    for peer in &peers.peers {
        let _ = send_request(
            peer,
            &identity,
            PeerMessage::new(
                "account.mailbox_key.announce",
                json!({ "key": mailbox_key_record }),
            ),
        )
        .await;
    }

    Ok(LoggedInAccount {
        username: username.to_string(),
        identity,
        cipher_key,
        mailbox_secret: mailbox_keys.secret,
        vfs_version,
        cache_dir,
        peers_reached,
        fresh_account,
        files_restored,
    })
}

async fn announce_claim(
    peer: &PeerEntry,
    identity: &NodeIdentity,
    claim: &rymnet_account::AccountClaimRecord,
    layout: &DataLayout,
) -> bool {
    let response = match send_request(
        peer,
        identity,
        PeerMessage::new("account.claim.announce", json!({ "claim": claim })),
    )
    .await
    {
        Ok(response) => response,
        Err(_) => return false,
    };
    if response.payload.get("status").and_then(|v| v.as_str()) == Some("conflict")
        && let Some(remote_claim) = response.payload.get("claim").filter(|v| !v.is_null())
        && let Ok(remote_claim) =
            serde_json::from_value::<rymnet_account::AccountClaimRecord>(remote_claim.clone())
    {
        let _ = merge_remote_claim(layout.account_claims_dir(), &remote_claim);
    }
    true
}

async fn pull_vfs(
    layout: &DataLayout,
    peers: &PeerConfig,
    identity: &NodeIdentity,
    cipher_key: &[u8; CIPHER_KEY_LEN],
    username: &str,
    cache_dir: &Path,
) -> Result<(u64, usize, bool)> {
    let mut best: Option<VfsPointerRecord> = None;
    if let Ok(Some(local)) = find_pointer(layout.account_pointers_dir(), username)
        && local.verify_owned_by(&identity.public_key).unwrap_or(false)
    {
        best = Some(local);
    }
    for peer in &peers.peers {
        let Ok(response) = send_request(
            peer,
            identity,
            PeerMessage::new("account.pointer.request", json!({ "username": username })),
        )
        .await
        else {
            continue;
        };
        let Some(pointer_value) = response.payload.get("pointer").filter(|v| !v.is_null()) else {
            continue;
        };
        let Ok(pointer) = serde_json::from_value::<VfsPointerRecord>(pointer_value.clone())
        else {
            continue;
        };
        if !pointer
            .verify_owned_by(&identity.public_key)
            .unwrap_or(false)
        {
            continue;
        }
        if best
            .as_ref()
            .is_none_or(|current| pointer.vfs_version > current.vfs_version)
        {
            best = Some(pointer);
        }
    }

    let _ = fs::remove_dir_all(cache_dir);
    fs::create_dir_all(cache_dir)?;

    let Some(pointer) = best else {
        return Ok((0, 0, true));
    };

    let store = ChunkStore::new(layout.chunks_root());
    let manifest_ciphertext =
        fetch_chunk(peers, identity, &pointer.manifest_chunk_hash, &store).await?;
    let manifest_bytes = decrypt(cipher_key, &manifest_ciphertext, &pointer.manifest_nonce)
        .context("decrypt vfs manifest; wrong password?")?;
    let manifest: PlainVfsManifest =
        serde_json::from_slice(&manifest_bytes).context("parse vfs manifest")?;

    let mut restored = 0usize;
    for file in &manifest.files {
        let ciphertext = fetch_chunk(peers, identity, &file.chunk_hash, &store).await?;
        let plaintext = decrypt(cipher_key, &ciphertext, &file.nonce)
            .with_context(|| format!("decrypt {}", file.path))?;
        let target = cache_dir.join(&file.path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&target, plaintext)?;
        restored += 1;
    }

    Ok((manifest.vfs_version, restored, false))
}

async fn fetch_chunk(
    peers: &PeerConfig,
    identity: &NodeIdentity,
    hash: &str,
    local_store: &ChunkStore,
) -> Result<Vec<u8>> {
    if local_store.exists(hash)? {
        return local_store.get(hash);
    }
    for peer in &peers.peers {
        let Ok(response) = send_request(
            peer,
            identity,
            PeerMessage::new("site.chunk.request", json!({ "hash": hash })),
        )
        .await
        else {
            continue;
        };
        if response.message_type != "site.chunk.response" {
            continue;
        }
        let Some(bytes_b64) = response.payload.get("bytes").and_then(|v| v.as_str()) else {
            continue;
        };
        if let Ok(bytes) = rymnet_network::b64_to_bytes(bytes_b64)
            && local_store.verify(hash, &bytes)
        {
            return Ok(bytes);
        }
    }
    anyhow::bail!("no peer could provide chunk {hash}")
}

pub struct PushSummary {
    pub peers_accepted: usize,
    pub peers_total: usize,
    pub files_pushed: usize,
    pub vfs_version: u64,
}

/// Same flow as `rymsh sync` / `rymweb`'s `sync` command: encrypt
/// everything under `cache_dir`, store ciphertext chunks, sign a new vfs
/// pointer, push to every configured peer.
pub async fn push(
    layout: &DataLayout,
    username: &str,
    identity: &NodeIdentity,
    cipher_key: &[u8; CIPHER_KEY_LEN],
    cache_dir: &Path,
    current_vfs_version: u64,
) -> Result<PushSummary> {
    let account_config = AppConfig::load(layout.config_path())
        .map(|config| config.account)
        .unwrap_or_default();

    let store = ChunkStore::new(layout.chunks_root());
    let mut files = Vec::new();
    let mut chunk_bytes: Vec<(String, Vec<u8>)> = Vec::new();

    for entry in WalkDir::new(cache_dir).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(cache_dir)?
            .to_string_lossy()
            .replace('\\', "/");
        let plaintext = fs::read(entry.path())?;
        let (ciphertext, nonce) = encrypt(cipher_key, &plaintext)?;
        let chunk_hash = store.put(&ciphertext)?;
        files.push(VfsFileEntry {
            path: relative,
            chunk_hash: chunk_hash.clone(),
            nonce,
            plaintext_size: plaintext.len() as u64,
            ciphertext_size: ciphertext.len() as u64,
        });
        chunk_bytes.push((chunk_hash, ciphertext));
    }

    let vfs_version = current_vfs_version + 1;
    let manifest = PlainVfsManifest {
        vfs_version,
        created_at: chrono::Utc::now(),
        files,
    };
    let manifest_bytes = serde_json::to_vec(&manifest)?;
    let (manifest_ciphertext, manifest_nonce) = encrypt(cipher_key, &manifest_bytes)?;
    let manifest_chunk_hash = store.put(&manifest_ciphertext)?;

    let total_bytes = manifest.total_ciphertext_size() + manifest_ciphertext.len() as u64;
    if !account_config.is_unlimited(username) {
        check_quota(total_bytes, account_config.quota_mb)?;
    }

    let pointer = VfsPointerRecord::create(
        username,
        identity,
        vfs_version,
        &manifest_chunk_hash,
        &manifest_nonce,
        total_bytes,
    )?;
    pointer.save(layout.account_pointers_dir())?;

    let local_claim = find_claim(layout.account_claims_dir(), username)?
        .context("missing local account claim; log in again")?;

    let peers = PeerConfig::load(layout.peers_path())?;
    let mut accepted = 0usize;
    for peer in &peers.peers {
        if push_to_peer(
            peer,
            identity,
            &local_claim,
            &pointer,
            &manifest_chunk_hash,
            &manifest_ciphertext,
            &chunk_bytes,
        )
        .await
        {
            accepted += 1;
        }
    }

    Ok(PushSummary {
        peers_accepted: accepted,
        peers_total: peers.peers.len(),
        files_pushed: manifest.files.len(),
        vfs_version,
    })
}

async fn push_to_peer(
    peer: &PeerEntry,
    identity: &NodeIdentity,
    claim: &rymnet_account::AccountClaimRecord,
    pointer: &VfsPointerRecord,
    manifest_chunk_hash: &str,
    manifest_ciphertext: &[u8],
    chunk_bytes: &[(String, Vec<u8>)],
) -> bool {
    if send_request(
        peer,
        identity,
        PeerMessage::new("account.claim.announce", json!({ "claim": claim })),
    )
    .await
    .is_err()
    {
        return false;
    }

    // Chunks are offered before the pointer is announced, not after: the
    // peer now refuses to accept a pointer for a version it hasn't seen
    // the matching chunks for (see rymnetd's account.pointer.announce
    // handler), so a peer's queryable "latest" pointer never ends up
    // referencing chunks it doesn't actually have.
    if offer_chunk(peer, identity, &pointer.username, pointer.vfs_version, manifest_chunk_hash, manifest_ciphertext)
        .await
        .is_err()
    {
        return false;
    }
    for (hash, bytes) in chunk_bytes {
        if offer_chunk(peer, identity, &pointer.username, pointer.vfs_version, hash, bytes)
            .await
            .is_err()
        {
            return false;
        }
    }

    let pointer_accepted = matches!(
        send_request(
            peer,
            identity,
            PeerMessage::new("account.pointer.announce", json!({ "pointer": pointer })),
        )
        .await,
        Ok(response) if response.payload.get("status").and_then(|v| v.as_str()) == Some("accepted")
    );
    pointer_accepted
}

async fn offer_chunk(
    peer: &PeerEntry,
    identity: &NodeIdentity,
    username: &str,
    vfs_version: u64,
    hash: &str,
    bytes: &[u8],
) -> Result<()> {
    let response = send_request(
        peer,
        identity,
        PeerMessage::new(
            "account.chunk.offer",
            json!({ "username": username, "vfs_version": vfs_version, "hash": hash, "bytes": rymnet_network::bytes_to_b64(bytes) }),
        ),
    )
    .await?;
    if response.payload.get("status").and_then(|v| v.as_str()) != Some("accepted") {
        anyhow::bail!("peer rejected chunk {hash}: {}", response.payload);
    }
    Ok(())
}

fn cache_dir_for_user(layout: &DataLayout, username: &str) -> PathBuf {
    layout.root.join("desktop-cache").join(username)
}

const HELLO_RUN: &str = include_str!("../../../starter_programs/hello.run");
const COUNTER_WIN: &str = include_str!("../../../starter_programs/counter.win");
const CHECKSUM_TOOL_RUN: &str = include_str!("../../../starter_programs/checksum_tool.run");
const NOTES_WIN: &str = include_str!("../../../starter_programs/notes.win");
const CLOCK_WIN: &str = include_str!("../../../starter_programs/clock.win");
const CALCULATOR_WIN: &str = include_str!("../../../starter_programs/calculator.win");
const MINESWEEPER_WIN: &str = include_str!("../../../starter_programs/minesweeper.win");

/// Every fresh account gets a `/Programs` folder with one basic example of
/// each RymScript program type, plus one example of each that exercises
/// the curated standard library (hashing/random/csv, and the JSON
/// helpers), so the SDK is discoverable the moment someone logs in for
/// the first time rather than only from a doc. `clock`/`calculator`/
/// `minesweeper` additionally exercise the `row`/`grid` layout
/// containers, button `color`/`enabled`, and the `now`/`now_ms` time
/// helpers -- proof, not just documentation, that those primitives are
/// enough to build something real before any visual designer is built
/// on top of them.
fn create_starter_programs(cache_dir: &Path) -> Result<()> {
    let programs_dir = cache_dir.join("Programs");
    fs::create_dir_all(&programs_dir)?;
    fs::write(programs_dir.join("hello.run"), HELLO_RUN)?;
    fs::write(programs_dir.join("counter.win"), COUNTER_WIN)?;
    fs::write(programs_dir.join("checksum_tool.run"), CHECKSUM_TOOL_RUN)?;
    fs::write(programs_dir.join("notes.win"), NOTES_WIN)?;
    fs::write(programs_dir.join("clock.win"), CLOCK_WIN)?;
    fs::write(programs_dir.join("calculator.win"), CALCULATOR_WIN)?;
    fs::write(programs_dir.join("minesweeper.win"), MINESWEEPER_WIN)?;
    Ok(())
}
