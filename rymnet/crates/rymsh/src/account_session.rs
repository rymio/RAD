use anyhow::{Context, Result};
use chrono::Utc;
use rymnet_account::{
    AccountClaimRecord, CIPHER_KEY_LEN, PlainVfsManifest, VfsFileEntry, VfsPointerRecord,
    check_quota, claim_username, decrypt, derive_account, encrypt, find_claim, find_pointer,
    merge_remote_claim, validate_username,
};
use rymnet_core::NodeIdentity;
use rymnet_mail::{MailboxKeyRecord, derive_mailbox_keypair};
use rymnet_network::{PeerMessage, b64_to_bytes, bytes_to_b64, send_request};
use rymnet_storage::{AppConfig, ChunkStore, DataLayout, PeerConfig, PeerEntry};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;
use x25519_dalek::StaticSecret;

pub struct AccountSession {
    pub username: String,
    pub identity: NodeIdentity,
    cipher_key: [u8; CIPHER_KEY_LEN],
    pub(crate) mailbox_secret: StaticSecret,
    pub vfs_version: u64,
    pub cache_dir: PathBuf,
}

pub struct PullSummary {
    pub peers_reached: usize,
    pub files_restored: usize,
    pub fresh_account: bool,
}

#[derive(Debug)]
pub struct PushSummary {
    pub peers_accepted: usize,
    pub peers_total: usize,
    pub files_pushed: usize,
    pub total_bytes: u64,
    pub vfs_version: u64,
}

/// Derives the account's identity+cipher key from username/password,
/// registers (or confirms) the username claim across configured peers, then
/// pulls the latest encrypted virtual computer from whichever peer has it.
/// Nothing here trusts local disk state as authoritative -- the network is
/// the source of truth, matching how `.rym` sites already work.
pub async fn login(
    layout: &DataLayout,
    username: &str,
    password: &str,
) -> Result<(AccountSession, PullSummary)> {
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
            "username '{username}' is already registered on the network under a different password; \
             choose a different username, or log in with the correct password"
        );
    }

    let cache_dir = layout.account_cache_dir(username);
    let (vfs_version, files_restored, fresh_account) =
        pull_vfs(layout, &peers, &identity, &cipher_key, username, &cache_dir).await?;
    if fresh_account {
        // Best-effort: a missing example program isn't worth failing
        // login over.
        let _ = create_starter_programs(&cache_dir);
    }

    let mailbox_keys = derive_mailbox_keypair(username, password)?;
    let mailbox_key_record = MailboxKeyRecord::create(username, &identity, &mailbox_keys.public)?;
    // Save locally too, independent of the peer announce below: otherwise
    // a peer that only knows this machine as its neighbor (rather than
    // wherever we happened to announce to) would have nowhere to fetch it
    // from, the same gap fixed earlier for the account claim/vfs pointer.
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

    Ok((
        AccountSession {
            username: username.to_string(),
            identity,
            cipher_key,
            mailbox_secret: mailbox_keys.secret,
            vfs_version,
            cache_dir,
        },
        PullSummary {
            peers_reached,
            files_restored,
            fresh_account,
        },
    ))
}

async fn announce_claim(
    peer: &PeerEntry,
    identity: &NodeIdentity,
    claim: &AccountClaimRecord,
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
        Err(err) => {
            eprintln!("account announce to {} failed: {err:#}", peer.name);
            return false;
        }
    };

    if response.payload.get("status").and_then(|v| v.as_str()) == Some("conflict")
        && let Some(remote_claim) = response.payload.get("claim").filter(|v| !v.is_null())
        && let Ok(remote_claim) = serde_json::from_value::<AccountClaimRecord>(remote_claim.clone())
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
    cache_dir: &PathBuf,
) -> Result<(u64, usize, bool)> {
    let mut best: Option<VfsPointerRecord> = None;
    if let Ok(Some(local)) = find_pointer(layout.account_pointers_dir(), username)
        && local.verify_owned_by(&identity.public_key).unwrap_or(false)
    {
        best = Some(local);
    }
    for peer in &peers.peers {
        let response = match send_request(
            peer,
            identity,
            PeerMessage::new("account.pointer.request", json!({ "username": username })),
        )
        .await
        {
            Ok(response) => response,
            Err(err) => {
                eprintln!("pointer lookup on {} failed: {err:#}", peer.name);
                continue;
            }
        };
        let Some(pointer_value) = response.payload.get("pointer").filter(|v| !v.is_null()) else {
            continue;
        };
        let Ok(pointer) = serde_json::from_value::<VfsPointerRecord>(pointer_value.clone()) else {
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
    fs::create_dir_all(cache_dir).with_context(|| format!("create {}", cache_dir.display()))?;

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
        let response = match send_request(
            peer,
            identity,
            PeerMessage::new("site.chunk.request", json!({ "hash": hash })),
        )
        .await
        {
            Ok(response) => response,
            Err(_) => continue,
        };
        if response.message_type != "site.chunk.response" {
            continue;
        }
        let Some(bytes_b64) = response.payload.get("bytes").and_then(|v| v.as_str()) else {
            continue;
        };
        if let Ok(bytes) = b64_to_bytes(bytes_b64)
            && local_store.verify(hash, &bytes)
        {
            return Ok(bytes);
        }
    }
    anyhow::bail!("no peer could provide chunk {hash}")
}

/// Re-pulls the latest vfs for an already-logged-in session (e.g. a `pull`
/// command run mid-session to grab changes pushed from another machine).
pub async fn pull(layout: &DataLayout, session: &mut AccountSession) -> Result<PullSummary> {
    let peers = PeerConfig::load(layout.peers_path())?;
    let (vfs_version, files_restored, fresh_account) = pull_vfs(
        layout,
        &peers,
        &session.identity,
        &session.cipher_key,
        &session.username,
        &session.cache_dir,
    )
    .await?;
    session.vfs_version = vfs_version;
    Ok(PullSummary {
        peers_reached: peers.peers.len(),
        files_restored,
        fresh_account,
    })
}

/// Encrypts every file under the session's local cache directory, stores the
/// ciphertext chunks, signs a new vfs pointer, and pushes everything to
/// configured peers so the account is reachable from other machines.
pub async fn push(layout: &DataLayout, session: &mut AccountSession) -> Result<PushSummary> {
    let account_config = AppConfig::load(layout.config_path())
        .map(|config| config.account)
        .unwrap_or_default();

    let store = ChunkStore::new(layout.chunks_root());
    let mut files = Vec::new();
    let mut chunk_bytes: Vec<(String, Vec<u8>)> = Vec::new();

    for entry in WalkDir::new(&session.cache_dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(&session.cache_dir)?
            .to_string_lossy()
            .replace('\\', "/");
        let plaintext = fs::read(entry.path())?;
        let (ciphertext, nonce) = encrypt(&session.cipher_key, &plaintext)?;
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

    let vfs_version = session.vfs_version + 1;
    let manifest = PlainVfsManifest {
        vfs_version,
        created_at: Utc::now(),
        files,
    };
    let manifest_bytes = serde_json::to_vec(&manifest)?;
    let (manifest_ciphertext, manifest_nonce) = encrypt(&session.cipher_key, &manifest_bytes)?;
    let manifest_chunk_hash = store.put(&manifest_ciphertext)?;

    let total_bytes = manifest.total_ciphertext_size() + manifest_ciphertext.len() as u64;
    if !account_config.is_unlimited(&session.username) {
        check_quota(total_bytes, account_config.quota_mb)?;
    }

    let pointer = VfsPointerRecord::create(
        &session.username,
        &session.identity,
        vfs_version,
        &manifest_chunk_hash,
        &manifest_nonce,
        total_bytes,
    )?;
    // Persist our own copy too, independent of quota bookkeeping (that only
    // applies to hosting *other* accounts' data): otherwise a peer that only
    // knows about us as its neighbor has nowhere to fetch this from later,
    // since only the receiving side of account.pointer.announce saves it.
    pointer.save(layout.account_pointers_dir())?;

    let local_claim = find_claim(layout.account_claims_dir(), &session.username)?
        .context("missing local account claim; log in again")?;
    local_claim.save(layout.account_claims_dir())?;

    let peers = PeerConfig::load(layout.peers_path())?;
    let mut accepted = 0usize;
    for peer in &peers.peers {
        if push_to_peer(
            peer,
            &session.identity,
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

    session.vfs_version = vfs_version;
    Ok(PushSummary {
        peers_accepted: accepted,
        peers_total: peers.peers.len(),
        files_pushed: manifest.files.len(),
        total_bytes,
        vfs_version,
    })
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
            json!({ "username": username, "vfs_version": vfs_version, "hash": hash, "bytes": bytes_to_b64(bytes) }),
        ),
    )
    .await?;
    if response.payload.get("status").and_then(|v| v.as_str()) != Some("accepted") {
        anyhow::bail!("peer rejected chunk {hash}: {}", response.payload);
    }
    Ok(())
}

async fn push_to_peer(
    peer: &PeerEntry,
    identity: &NodeIdentity,
    claim: &AccountClaimRecord,
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
    if offer_chunk(
        peer,
        identity,
        &pointer.username,
        pointer.vfs_version,
        manifest_chunk_hash,
        manifest_ciphertext,
    )
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

const HELLO_RUN: &str = include_str!("../../../starter_programs/hello.run");
const COUNTER_WIN: &str = include_str!("../../../starter_programs/counter.win");
const CHECKSUM_TOOL_RUN: &str = include_str!("../../../starter_programs/checksum_tool.run");
const NOTES_WIN: &str = include_str!("../../../starter_programs/notes.win");
const CLOCK_WIN: &str = include_str!("../../../starter_programs/clock.win");
const CALCULATOR_WIN: &str = include_str!("../../../starter_programs/calculator.win");
const MINESWEEPER_WIN: &str = include_str!("../../../starter_programs/minesweeper.win");

/// Every fresh account gets a `/Programs` folder with one basic example
/// of each RymScript program type, plus one example of each that
/// exercises the curated standard library (shared verbatim with rymwin's
/// copy of this same logic, via the same source files), so the SDK is
/// discoverable the moment someone logs in for the first time. The
/// `.win` examples can't actually be *run* from here (no graphical
/// desktop in `rymsh`), but the account's vfs is shared with `rymwin`,
/// so they still need to exist here too.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The quota check runs before any peer is contacted, so this exercises
    /// real enforcement without needing a live network.
    #[tokio::test]
    async fn push_rejects_over_quota_vfs() {
        let dir = std::env::temp_dir().join(format!(
            "rymnet-account-session-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let layout = DataLayout::from_root(dir.clone());
        layout.ensure_all().expect("ensure_all");

        let mut config = AppConfig::default();
        config.account.quota_mb = 1;
        config.save_new(layout.config_path()).expect("save config");

        let derived = derive_account("quotauser", "quota-pass").expect("derive");
        claim_username(layout.account_claims_dir(), "quotauser", &derived.identity).expect("claim");

        let cache_dir = layout.account_cache_dir("quotauser");
        fs::create_dir_all(&cache_dir).expect("create cache dir");
        fs::write(cache_dir.join("big.bin"), vec![0u8; 2_000_000]).expect("write big file");

        let mailbox_keys =
            derive_mailbox_keypair("quotauser", "quota-pass").expect("derive mailbox");
        let mut session = AccountSession {
            username: "quotauser".to_string(),
            identity: derived.identity,
            cipher_key: derived.cipher_key,
            mailbox_secret: mailbox_keys.secret,
            vfs_version: 0,
            cache_dir,
        };

        let err = push(&layout, &mut session)
            .await
            .expect_err("push should fail over quota");
        assert!(err.to_string().contains("quota"), "unexpected error: {err}");

        let _ = fs::remove_dir_all(&dir);
    }
}
