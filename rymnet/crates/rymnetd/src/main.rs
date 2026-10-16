use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use rymnet_account::{AccountClaimRecord, VfsPointerRecord, check_quota, find_pointer};
use rymnet_board::{Comment, Post, PostSummary, board_dir_name, validate_board};
use rymnet_chat::{ChatMessage, channel_dir_name, validate_channel};
use rymnet_core::NodeIdentity;
use rymnet_domain::{DomainClaimRecord, find_claim, list_claims};
use rymnet_mail::{MailEnvelope, MailboxKeyRecord, find_mailbox_key};
use rymnet_network::{
    Handler, PeerMessage, PeerSessionInfo, b64_to_bytes, bytes_to_b64, unix_millis_now,
};
use rymnet_search::{
    SearchRecord, default_index_root, default_records_root, index_records, list_search_records,
    rebuild_index, save_verified_search_record,
};
use rymnet_site::{SiteManifest, load_latest_manifest, save_replicated_manifest, set_pin};
use rymnet_storage::{
    AccountConfig, AppConfig, BoardConfig, ChatConfig, ChunkStore, DataLayout, FileConfig,
    MailConfig, PeerConfig,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs;
use std::sync::Arc;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(name = "rymnetd", version, about = "Rymnet daemon")]
struct Cli {
    #[arg(long, value_name = "FILE")]
    config: Option<String>,

    #[arg(long, value_name = "DIR")]
    data_dir: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Run,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive("rymnetd=info".parse()?)
                .add_directive("rymnet_gateway=info".parse()?),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();
    match cli.command {
        Commands::Run => run(cli).await?,
    }
    Ok(())
}

async fn run(cli: Cli) -> Result<()> {
    let layout = match cli.data_dir.as_deref() {
        Some(dir) => DataLayout::from_root(rymnet_storage::layout::expand_tilde(dir)?),
        None => DataLayout::default()?,
    };
    let config_path = match cli.config {
        Some(path) => rymnet_storage::layout::expand_tilde(&path)?,
        None => layout.config_path(),
    };
    let config = AppConfig::load(&config_path).with_context(|| {
        format!(
            "load config {}; run `rym init` first",
            config_path.display()
        )
    })?;
    let identity = NodeIdentity::load(layout.identity_path())
        .with_context(|| "load node identity; run `rym init` first")?;

    info!("rymnetd starting");
    info!("node id: {}", identity.node_id()?);
    info!("peer listener: {}", config.node.listen_addr);
    if config.gateway.enabled {
        info!("gateway: http://{}", config.gateway.listen_addr);
    }
    if config.admin.enabled {
        info!("admin: http://{}", config.admin.listen_addr);
    }
    info!(
        "search: {}",
        if config.search.enabled {
            "enabled"
        } else {
            "disabled"
        }
    );
    info!(
        "replication: {}",
        if config.replication.enabled {
            "enabled"
        } else {
            "disabled"
        }
    );
    info!("connected peers: 0");
    if config.search.enabled && config.search.auto_index {
        match rebuild_index(
            default_index_root(layout.search_root()),
            layout.site_root(),
            layout.chunks_root(),
        ) {
            Ok(count) => info!("search indexed documents: {count}"),
            Err(err) => warn!("search index rebuild failed: {err:#}"),
        }
    }

    let gateway_task = if config.gateway.enabled {
        let gateway_layout = layout.clone();
        let gateway_addr = config.gateway.listen_addr.clone();
        Some(tokio::spawn(async move {
            if let Err(err) = rymnet_gateway::serve(gateway_layout, &gateway_addr).await {
                warn!("gateway stopped: {err:#}");
            }
        }))
    } else {
        None
    };

    let peer_task = if config.replication.enabled {
        let peer_layout = layout.clone();
        let peer_identity = identity.clone();
        let listen_addr = config.node.listen_addr.clone();
        let handler = peer_handler(
            peer_layout,
            config.account.clone(),
            config.mail.clone(),
            config.chat.clone(),
            config.board.clone(),
            config.files.clone(),
            config.node.share_peer_list,
        );
        Some(tokio::spawn(async move {
            if let Err(err) = rymnet_network::serve(&listen_addr, peer_identity, handler).await {
                warn!("peer listener stopped: {err:#}");
            }
        }))
    } else {
        None
    };

    let resync_task = if config.replication.enabled {
        info!(
            "periodic resync: every {}s",
            config.replication.resync_interval_secs
        );
        let resync_layout = layout.clone();
        let resync_identity = identity.clone();
        let interval_secs = config.replication.resync_interval_secs.max(1);
        Some(tokio::spawn(async move {
            resync_loop(resync_layout, resync_identity, interval_secs).await;
        }))
    } else {
        None
    };

    info!("rymnetd running. Press Ctrl-C to stop.");

    tokio::signal::ctrl_c().await.context("wait for Ctrl-C")?;
    if let Some(task) = gateway_task {
        task.abort();
    }
    if let Some(task) = peer_task {
        task.abort();
    }
    if let Some(task) = resync_task {
        task.abort();
    }
    info!("rymnetd stopping");
    Ok(())
}

/// Every `interval_secs`, re-pushes everything this node has locally --
/// domain claims + their site manifests/chunks + their search records,
/// account claims + their vfs pointers + their vfs chunks, mailbox keys,
/// and relayed mail -- to every configured peer. This covers both a
/// peer that was offline when something was originally published/sent,
/// and one that joined the mesh afterward via `rym peer connect`: either
/// way it converges on the next tick instead of staying permanently
/// missing that content. Sites and accounts are re-pushed regardless of
/// whether *this* node originated them -- a node just hosting a replica
/// re-shares it to its own peers too, so content actually spreads
/// transitively across the mesh instead of only ever radiating out from
/// the original publisher's own peer list.
///
/// This is unconditional re-push, not a smarter "diff against what the
/// peer already has" reconciliation: simple and correct, at the cost of
/// re-sending unchanged content on every tick. Fine for the small sites,
/// modest account/mail volumes, and modest peer counts this project
/// targets; would need rethinking (e.g. a lighter "do you have version N
/// of X?" probe first) if any of those grow large.
async fn resync_loop(layout: DataLayout, identity: NodeIdentity, interval_secs: u64) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
    // Default `Burst` behavior re-fires every missed tick back-to-back with
    // no pacing once a slow round (e.g. many peers/accounts, or a peer with
    // real network latency) overruns the interval -- observed live during
    // testing: one slow tick caused half a dozen rapid-fire re-pushes to a
    // peer in under two seconds. `Delay` instead waits a full interval from
    // when the previous round *finished*, which is what a background
    // resync job pushing real data to real peers should do.
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        interval.tick().await;
        let peers = match PeerConfig::load(layout.peers_path()) {
            Ok(peers) if !peers.peers.is_empty() => peers,
            Ok(_) => continue,
            Err(err) => {
                warn!("periodic resync: failed to load peers.toml: {err:#}");
                continue;
            }
        };
        let sites = resync_sites(&layout, &identity, &peers).await;
        let accounts = resync_accounts(&layout, &identity, &peers).await;
        let mail = resync_mail(&layout, &identity, &peers).await;
        let chat = resync_chat(&layout, &identity, &peers).await;
        let board = resync_board(&layout, &identity, &peers).await;
        if sites > 0 || accounts > 0 || mail > 0 || chat > 0 || board > 0 {
            info!(
                "periodic resync: {sites} site(s), {accounts} account(s), {mail} mail message(s), \
                 {chat} chat message(s), {board} board post(s) re-announced to {} peer(s)",
                peers.peers.len()
            );
        }
    }
}

/// Re-offers every domain this node has a complete local copy of
/// (owned or merely hosted as a replica -- both leave the same claim/
/// manifest/chunks on disk, see `accept_replica_offer`) to every peer.
async fn resync_sites(layout: &DataLayout, identity: &NodeIdentity, peers: &PeerConfig) -> usize {
    let Ok(claims) = list_claims(layout.domain_claims_dir()) else {
        return 0;
    };
    let store = ChunkStore::new(layout.chunks_root());
    let all_records = list_search_records(default_records_root(layout.search_root())).unwrap_or_default();
    let mut resynced = 0usize;

    for claim in &claims {
        if !claim.verify().unwrap_or(false) || claim.is_expired() {
            continue;
        }
        let Ok(Some(manifest)) = load_latest_manifest(&claim.domain, layout.site_root()) else {
            continue;
        };
        if !manifest.verify(&claim.owner_public_key).unwrap_or(false) {
            continue;
        }
        let Ok(chunks) = manifest
            .files
            .iter()
            .map(|file| {
                let bytes = store.get(&file.hash)?;
                anyhow::Ok(json!({"hash": file.hash, "bytes": bytes_to_b64(&bytes)}))
            })
            .collect::<Result<Vec<_>>>()
        else {
            continue;
        };
        let replica_payload = json!({ "claim": claim, "manifest": &manifest, "chunks": chunks });
        for peer in &peers.peers {
            let _ = rymnet_network::send_request(
                peer,
                identity,
                PeerMessage::new("replica.offer", replica_payload.clone()),
            )
            .await;
        }

        for record in all_records.iter().filter(|record| record.domain == claim.domain) {
            for peer in &peers.peers {
                let _ = rymnet_network::send_request(
                    peer,
                    identity,
                    PeerMessage::new("search.record.announce", json!({"claim": claim, "record": record})),
                )
                .await;
            }
        }
        resynced += 1;
    }
    resynced
}

/// Re-offers every account this node has a claim for: the claim, its
/// latest vfs pointer, the (opaque, still-encrypted) manifest chunk, and
/// every file chunk this node has actually accepted for that pointer's
/// version (tracked in `HostedLedger.chunk_hashes` since `find_pointer`
/// alone can't tell us which chunk hashes belong to a user -- the
/// manifest listing them is encrypted with a key this node never has).
/// Also re-announces that account's mailbox key if one is on file.
async fn resync_accounts(layout: &DataLayout, identity: &NodeIdentity, peers: &PeerConfig) -> usize {
    let Ok(claims) = rymnet_account::list_claims(layout.account_claims_dir()) else {
        return 0;
    };
    let store = ChunkStore::new(layout.chunks_root());
    let mut resynced = 0usize;

    for claim in &claims {
        if !claim.verify().unwrap_or(false) {
            continue;
        }
        for peer in &peers.peers {
            let _ = rymnet_network::send_request(
                peer,
                identity,
                PeerMessage::new("account.claim.announce", json!({"claim": claim})),
            )
            .await;
        }

        if let Ok(Some(key)) = find_mailbox_key(layout.mailbox_keys_dir(), &claim.username)
            && key.verify().unwrap_or(false)
        {
            for peer in &peers.peers {
                let _ = rymnet_network::send_request(
                    peer,
                    identity,
                    PeerMessage::new("account.mailbox_key.announce", json!({"key": key})),
                )
                .await;
            }
        }

        let Ok(Some(pointer)) = find_pointer(layout.account_pointers_dir(), &claim.username) else {
            continue;
        };
        if !pointer.verify_owned_by(&claim.owner_public_key).unwrap_or(false) {
            continue;
        }

        // Chunks first, pointer last -- a peer now rejects
        // account.pointer.announce until it has seen this version's
        // manifest chunk (see the handler), so re-announcing the pointer
        // before re-offering chunks would just get "chunks_missing" back
        // from any peer that doesn't already have this account's data.
        let mut hashes = vec![pointer.manifest_chunk_hash.clone()];
        if let Ok(Some(ledger)) = load_hosted_ledger(layout, &claim.username)
            && ledger.vfs_version == pointer.vfs_version
        {
            hashes.extend(ledger.chunk_hashes.iter().cloned());
        }
        for hash in hashes {
            let Ok(bytes) = store.get(&hash) else { continue };
            if !store.verify(&hash, &bytes) {
                continue;
            }
            let payload = json!({
                "username": claim.username,
                "vfs_version": pointer.vfs_version,
                "hash": hash,
                "bytes": bytes_to_b64(&bytes),
            });
            for peer in &peers.peers {
                let _ =
                    rymnet_network::send_request(peer, identity, PeerMessage::new("account.chunk.offer", payload.clone()))
                        .await;
            }
        }
        for peer in &peers.peers {
            let _ = rymnet_network::send_request(
                peer,
                identity,
                PeerMessage::new("account.pointer.announce", json!({"pointer": &pointer})),
            )
            .await;
        }
        resynced += 1;
    }
    resynced
}

/// Re-delivers every mail message this node is relaying, for every
/// recipient it holds an inbox for, to every peer. `mail.deliver` is
/// idempotent (deduped by envelope id on the receiving side), so this
/// is safe to repeat indefinitely.
async fn resync_mail(layout: &DataLayout, identity: &NodeIdentity, peers: &PeerConfig) -> usize {
    let inbox_root = layout.mail_root().join("inbox");
    let Ok(usernames) = fs::read_dir(&inbox_root) else {
        return 0;
    };
    let mut resynced = 0usize;
    for username_entry in usernames.flatten() {
        if !username_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let Ok(messages) = fs::read_dir(username_entry.path()) else {
            continue;
        };
        for message_entry in messages.flatten() {
            if message_entry.path().extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let Ok(bytes) = fs::read(message_entry.path()) else {
                continue;
            };
            let Ok(envelope) = serde_json::from_slice::<MailEnvelope>(&bytes) else {
                continue;
            };
            if !envelope.verify_signature().unwrap_or(false) {
                continue;
            }
            for peer in &peers.peers {
                let _ = rymnet_network::send_request(
                    peer,
                    identity,
                    PeerMessage::new("mail.deliver", json!({"envelope": &envelope})),
                )
                .await;
            }
            resynced += 1;
        }
    }
    resynced
}

/// Re-delivers every channel message this node is relaying, for every
/// channel it holds any messages for, to every peer. `chat.channel.deliver`
/// is idempotent (deduped by message id on the receiving side, exactly
/// like `mail.deliver`), so this is safe to repeat indefinitely.
async fn resync_chat(layout: &DataLayout, identity: &NodeIdentity, peers: &PeerConfig) -> usize {
    let channels_root = layout.chat_root().join("channels");
    let Ok(channel_dirs) = fs::read_dir(&channels_root) else {
        return 0;
    };
    let mut resynced = 0usize;
    for channel_entry in channel_dirs.flatten() {
        if !channel_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let Ok(messages) = fs::read_dir(channel_entry.path()) else {
            continue;
        };
        for message_entry in messages.flatten() {
            if message_entry.path().extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let Ok(bytes) = fs::read(message_entry.path()) else {
                continue;
            };
            let Ok(message) = serde_json::from_slice::<ChatMessage>(&bytes) else {
                continue;
            };
            if !message.verify_signature().unwrap_or(false) {
                continue;
            }
            for peer in &peers.peers {
                let _ = rymnet_network::send_request(
                    peer,
                    identity,
                    PeerMessage::new("chat.channel.deliver", json!({"message": &message})),
                )
                .await;
            }
            resynced += 1;
        }
    }
    resynced
}

/// Re-delivers every post this node is relaying, for every board it
/// holds any posts for, to every peer. Same idempotent-by-id shape as
/// `resync_chat` -- safe to repeat indefinitely. Comments are not
/// resynced in this first pass, the same scope-limiting call this
/// feature's own doc comments already make elsewhere.
async fn resync_board(layout: &DataLayout, identity: &NodeIdentity, peers: &PeerConfig) -> usize {
    let boards_root = layout.board_root().join("boards");
    let Ok(board_dirs) = fs::read_dir(&boards_root) else {
        return 0;
    };
    let mut resynced = 0usize;
    for board_entry in board_dirs.flatten() {
        if !board_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let Ok(dir_name) = board_entry.file_name().into_string() else {
            continue;
        };
        let Ok(posts) = fs::read_dir(layout.board_posts_dir(&dir_name)) else {
            continue;
        };
        for post_entry in posts.flatten() {
            if !post_entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            if post_entry.path().extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let Ok(bytes) = fs::read(post_entry.path()) else {
                continue;
            };
            let Ok(post) = serde_json::from_slice::<Post>(&bytes) else {
                continue;
            };
            if !post.verify_signature().unwrap_or(false) {
                continue;
            }
            for peer in &peers.peers {
                let _ = rymnet_network::send_request(
                    peer,
                    identity,
                    PeerMessage::new("board.post.deliver", json!({"post": &post})),
                )
                .await;
            }
            resynced += 1;
        }
    }
    resynced
}

#[allow(clippy::too_many_arguments)]
fn peer_handler(
    layout: DataLayout,
    account: AccountConfig,
    mail: MailConfig,
    chat: ChatConfig,
    board: BoardConfig,
    files: FileConfig,
    share_peer_list: bool,
) -> Handler {
    Arc::new(move |info, request| {
        let layout = layout.clone();
        let account = account.clone();
        let mail = mail.clone();
        let chat = chat.clone();
        let board = board.clone();
        let files = files.clone();
        Box::pin(async move {
            handle_peer_message(layout, account, mail, chat, board, files, share_peer_list, info, request).await
        })
    })
}

#[allow(clippy::too_many_arguments)]
async fn handle_peer_message(
    layout: DataLayout,
    account: AccountConfig,
    mail: MailConfig,
    chat: ChatConfig,
    board: BoardConfig,
    files: FileConfig,
    share_peer_list: bool,
    _info: PeerSessionInfo,
    request: PeerMessage,
) -> Result<PeerMessage> {
    match request.message_type.as_str() {
        "peer.status" => Ok(PeerMessage::response_to(
            &request,
            "peer.status",
            json!({"status": "ok"}),
        )),
        "time.sync" => Ok(PeerMessage::response_to(
            &request,
            "time.sync",
            json!({"server_time_ms": unix_millis_now()}),
        )),
        "peer.list.request" => {
            if !share_peer_list {
                return Ok(PeerMessage::response_to(
                    &request,
                    "peer.list.response",
                    json!({"shared": false, "peers": []}),
                ));
            }
            let peers = PeerConfig::load(layout.peers_path())?;
            Ok(PeerMessage::response_to(
                &request,
                "peer.list.response",
                json!({"shared": true, "peers": peers.peers}),
            ))
        }
        "site.manifest.request" => {
            let domain = request
                .payload
                .get("domain")
                .and_then(|value| value.as_str())
                .context("missing domain")?;
            let claim = find_claim(layout.domain_claims_dir(), domain)?
                .with_context(|| format!("no claim for {domain}"))?;
            if !claim.verify()? || claim.is_expired() {
                anyhow::bail!("invalid domain claim for {domain}");
            }
            let manifest = load_latest_manifest(domain, layout.site_root())?
                .with_context(|| format!("no manifest for {domain}"))?;
            if !manifest.verify(&claim.owner_public_key)? {
                anyhow::bail!("invalid manifest signature for {domain}");
            }
            Ok(PeerMessage::response_to(
                &request,
                "site.manifest.response",
                json!({
                    "claim": claim,
                    "manifest": manifest,
                    "manifest_hash": manifest.hash()?,
                }),
            ))
        }
        "site.chunk.request" => {
            let hash = request
                .payload
                .get("hash")
                .and_then(|value| value.as_str())
                .context("missing hash")?;
            let store = ChunkStore::new(layout.chunks_root());
            let bytes = store.get(hash)?;
            if !store.verify(hash, &bytes) {
                anyhow::bail!("chunk hash verification failed");
            }
            Ok(PeerMessage::response_to(
                &request,
                "site.chunk.response",
                json!({"hash": hash, "bytes": bytes_to_b64(&bytes)}),
            ))
        }
        "search.record.announce" => {
            let claim: DomainClaimRecord = serde_json::from_value(
                request
                    .payload
                    .get("claim")
                    .context("missing claim")?
                    .clone(),
            )?;
            if !claim.verify()? || claim.is_expired() {
                anyhow::bail!("invalid search record claim");
            }
            let record: SearchRecord = serde_json::from_value(
                request
                    .payload
                    .get("record")
                    .context("missing record")?
                    .clone(),
            )?;
            if record.domain != claim.domain {
                anyhow::bail!("search record domain mismatch");
            }
            save_verified_search_record(
                default_records_root(layout.search_root()),
                &record,
                &claim.owner_public_key,
            )?;
            index_records(
                default_index_root(layout.search_root()),
                std::slice::from_ref(&record),
            )?;
            Ok(PeerMessage::response_to(
                &request,
                "search.record.response",
                json!({"status": "accepted", "record_hash": record.hash()?}),
            ))
        }
        "search.query" => {
            let query = request
                .payload
                .get("query")
                .and_then(|value| value.as_str())
                .context("missing query")?
                .to_ascii_lowercase();
            let domain_filter = request
                .payload
                .get("domain")
                .and_then(|value| value.as_str())
                .map(str::to_string);
            let limit = request
                .payload
                .get("limit")
                .and_then(|value| value.as_u64())
                .unwrap_or(10) as usize;
            let records = list_search_records(default_records_root(layout.search_root()))?
                .into_iter()
                .filter(|record| {
                    domain_filter
                        .as_deref()
                        .is_none_or(|domain| record.domain == domain)
                })
                .filter(|record| search_record_matches(record, &query))
                .filter_map(|record| {
                    let claim = find_claim(layout.domain_claims_dir(), &record.domain)
                        .ok()
                        .flatten()?;
                    if claim.verify().ok()? && record.verify(&claim.owner_public_key).ok()? {
                        Some(json!({"claim": claim, "record": record}))
                    } else {
                        None
                    }
                })
                .take(limit)
                .collect::<Vec<_>>();
            Ok(PeerMessage::response_to(
                &request,
                "search.response",
                json!({"records": records}),
            ))
        }
        "replica.offer" => {
            accept_replica_offer(&layout, &request.payload)?;
            Ok(PeerMessage::response_to(
                &request,
                "replica.status",
                json!({"status": "complete"}),
            ))
        }
        "site.manifest.announce" => Ok(PeerMessage::response_to(
            &request,
            "replica.status",
            json!({"status": "announce-received"}),
        )),
        "account.claim.request" => {
            let username = request
                .payload
                .get("username")
                .and_then(|value| value.as_str())
                .context("missing username")?;
            let claim = rymnet_account::find_claim(layout.account_claims_dir(), username)?;
            Ok(PeerMessage::response_to(
                &request,
                "account.claim.response",
                json!({"claim": claim}),
            ))
        }
        "account.claim.announce" => {
            let claim: AccountClaimRecord = serde_json::from_value(
                request
                    .payload
                    .get("claim")
                    .context("missing claim")?
                    .clone(),
            )?;
            match rymnet_account::merge_remote_claim(layout.account_claims_dir(), &claim) {
                Ok(merged) => Ok(PeerMessage::response_to(
                    &request,
                    "account.claim.response",
                    json!({"status": "accepted", "owner_public_key": merged.owner_public_key}),
                )),
                Err(err) => {
                    let winning =
                        rymnet_account::find_claim(layout.account_claims_dir(), &claim.username)?;
                    Ok(PeerMessage::response_to(
                        &request,
                        "account.claim.response",
                        json!({"status": "conflict", "error": err.to_string(), "claim": winning}),
                    ))
                }
            }
        }
        "account.pointer.request" => {
            let username = request
                .payload
                .get("username")
                .and_then(|value| value.as_str())
                .context("missing username")?;
            let pointer = find_pointer(layout.account_pointers_dir(), username)?;
            Ok(PeerMessage::response_to(
                &request,
                "account.pointer.response",
                json!({"pointer": pointer}),
            ))
        }
        "account.pointer.announce" => {
            let pointer: VfsPointerRecord = serde_json::from_value(
                request
                    .payload
                    .get("pointer")
                    .context("missing pointer")?
                    .clone(),
            )?;
            let claim = rymnet_account::find_claim(layout.account_claims_dir(), &pointer.username)?
                .context("unknown username; announce an account claim first")?;
            if !claim.verify()? {
                anyhow::bail!("stored account claim signature invalid");
            }
            if !pointer.verify_owned_by(&claim.owner_public_key)? {
                anyhow::bail!("vfs pointer is not signed by the claimed account owner");
            }
            if !account.is_unlimited(&pointer.username) {
                if let Err(err) = check_quota(pointer.total_bytes, account.quota_mb) {
                    return Ok(PeerMessage::response_to(
                        &request,
                        "account.pointer.response",
                        json!({"status": "quota_exceeded", "error": err.to_string()}),
                    ));
                }
            }
            // Chunks are always offered before the pointer that references
            // them is announced (see `push_to_peer` in the clients) -- so
            // by the time this runs, this version's manifest chunk should
            // already be sitting in this node's ledger. If it isn't, the
            // upload never finished; accepting the pointer anyway would
            // make this node advertise a "latest" version nobody, including
            // this node, can actually serve chunks for.
            let has_manifest_chunk = load_hosted_ledger(&layout, &pointer.username)?.is_some_and(|ledger| {
                ledger.vfs_version == pointer.vfs_version
                    && ledger.chunk_hashes.contains(&pointer.manifest_chunk_hash)
            });
            if !has_manifest_chunk {
                return Ok(PeerMessage::response_to(
                    &request,
                    "account.pointer.response",
                    json!({"status": "chunks_missing", "vfs_version": pointer.vfs_version}),
                ));
            }
            let is_newer = match find_pointer(layout.account_pointers_dir(), &pointer.username)? {
                Some(existing) => pointer.vfs_version > existing.vfs_version,
                None => true,
            };
            if is_newer {
                pointer.save(layout.account_pointers_dir())?;
            }
            Ok(PeerMessage::response_to(
                &request,
                "account.pointer.response",
                json!({"status": "accepted", "vfs_version": pointer.vfs_version}),
            ))
        }
        "account.chunk.offer" => {
            let username = request
                .payload
                .get("username")
                .and_then(|value| value.as_str())
                .context("missing username")?;
            let vfs_version = request
                .payload
                .get("vfs_version")
                .and_then(|value| value.as_u64())
                .context("missing vfs_version")?;
            let hash = request
                .payload
                .get("hash")
                .and_then(|value| value.as_str())
                .context("missing hash")?;
            let bytes = b64_to_bytes(
                request
                    .payload
                    .get("bytes")
                    .and_then(|value| value.as_str())
                    .context("missing bytes")?,
            )?;
            let store = ChunkStore::new(layout.chunks_root());
            if !store.verify(hash, &bytes) {
                anyhow::bail!("chunk hash verification failed");
            }
            // The chunk offer -- not the pointer announce -- is what opens
            // this account's ledger for a given vfs_version now (see the
            // comment on `account.pointer.announce` above): the first
            // offer for a version this node hasn't seen before starts a
            // fresh accounting window, same as `reset_hosted_ledger` used
            // to do explicitly.
            let mut ledger = match load_hosted_ledger(&layout, username)? {
                Some(existing) if existing.vfs_version == vfs_version => existing,
                _ => HostedLedger { vfs_version, accepted_bytes: 0, chunk_hashes: Default::default() },
            };
            if store.exists(hash)? {
                // Already on disk (a prior offer, or another user's
                // identical chunk) -- no new bytes, so no quota check,
                // but still record the hash under this username so
                // periodic resync knows to keep re-offering it.
                if ledger.chunk_hashes.insert(hash.to_string()) {
                    save_hosted_ledger(&layout, username, &ledger)?;
                }
                return Ok(PeerMessage::response_to(
                    &request,
                    "account.chunk.response",
                    json!({"status": "accepted", "hash": hash}),
                ));
            }
            if !account.is_unlimited(username)
                && ledger.accepted_bytes + bytes.len() as u64
                    > rymnet_account::quota_bytes(account.quota_mb)
            {
                return Ok(PeerMessage::response_to(
                    &request,
                    "account.chunk.response",
                    json!({"status": "quota_exceeded", "hash": hash}),
                ));
            }
            store.put(&bytes)?;
            ledger.accepted_bytes += bytes.len() as u64;
            ledger.chunk_hashes.insert(hash.to_string());
            save_hosted_ledger(&layout, username, &ledger)?;
            Ok(PeerMessage::response_to(
                &request,
                "account.chunk.response",
                json!({"status": "accepted", "hash": hash}),
            ))
        }
        "account.mailbox_key.request" => {
            let username = request
                .payload
                .get("username")
                .and_then(|value| value.as_str())
                .context("missing username")?;
            let key = find_mailbox_key(layout.mailbox_keys_dir(), username)?;
            Ok(PeerMessage::response_to(
                &request,
                "account.mailbox_key.response",
                json!({"key": key}),
            ))
        }
        "account.mailbox_key.announce" => {
            let key: MailboxKeyRecord =
                serde_json::from_value(request.payload.get("key").context("missing key")?.clone())?;
            if !key.verify()? {
                anyhow::bail!("mailbox key signature invalid");
            }
            let claim = rymnet_account::find_claim(layout.account_claims_dir(), &key.username)?
                .context("unknown username; announce an account claim first")?;
            if !claim.verify()? || claim.owner_public_key != key.owner_public_key {
                anyhow::bail!("mailbox key is not signed by the claimed account owner");
            }
            key.save(layout.mailbox_keys_dir())?;
            Ok(PeerMessage::response_to(
                &request,
                "account.mailbox_key.response",
                json!({"status": "accepted"}),
            ))
        }
        "mail.deliver" => {
            let envelope: MailEnvelope = serde_json::from_value(
                request
                    .payload
                    .get("envelope")
                    .context("missing envelope")?
                    .clone(),
            )?;
            if !envelope.verify_signature()? {
                anyhow::bail!("mail envelope signature invalid");
            }
            let size = envelope.approx_size();
            let mut ledger = load_mail_ledger(&layout, &envelope.to)?.unwrap_or_default();
            if !account.is_unlimited(&envelope.to)
                && ledger.accepted_bytes + size > rymnet_account::quota_bytes(mail.quota_mb)
            {
                return Ok(PeerMessage::response_to(
                    &request,
                    "mail.deliver.response",
                    json!({"status": "quota_exceeded"}),
                ));
            }
            let dir = layout.mail_inbox_dir(&envelope.to);
            fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
            let path = dir.join(format!("{}.json", envelope.id));
            if !path.exists() {
                fs::write(&path, serde_json::to_vec_pretty(&envelope)?)
                    .with_context(|| format!("write {}", path.display()))?;
                ledger.accepted_bytes += size;
                save_mail_ledger(&layout, &envelope.to, &ledger)?;
            }
            Ok(PeerMessage::response_to(
                &request,
                "mail.deliver.response",
                json!({"status": "accepted", "id": envelope.id}),
            ))
        }
        "mail.inbox.request" => {
            let username = request
                .payload
                .get("username")
                .and_then(|value| value.as_str())
                .context("missing username")?;
            let dir = layout.mail_inbox_dir(username);
            let mut envelopes = Vec::new();
            if dir.exists() {
                for entry in
                    fs::read_dir(&dir).with_context(|| format!("read {}", dir.display()))?
                {
                    let entry = entry?;
                    if entry.path().extension().and_then(|ext| ext.to_str()) != Some("json") {
                        continue;
                    }
                    let bytes = fs::read(entry.path())?;
                    if let Ok(envelope) = serde_json::from_slice::<MailEnvelope>(&bytes) {
                        envelopes.push(envelope);
                    }
                }
            }
            Ok(PeerMessage::response_to(
                &request,
                "mail.inbox.response",
                json!({"envelopes": envelopes}),
            ))
        }
        "chat.channel.deliver" => {
            let message: ChatMessage = serde_json::from_value(
                request
                    .payload
                    .get("message")
                    .context("missing message")?
                    .clone(),
            )?;
            if !message.verify_signature()? {
                anyhow::bail!("chat message signature invalid");
            }
            validate_channel(&message.channel)?;
            let dir_name = channel_dir_name(&message.channel)?;
            let size = message.approx_size();
            let mut ledger = load_chat_ledger(&layout, &dir_name)?.unwrap_or_default();
            if ledger.accepted_bytes + size > rymnet_account::quota_bytes(chat.quota_mb) {
                return Ok(PeerMessage::response_to(
                    &request,
                    "chat.channel.deliver.response",
                    json!({"status": "quota_exceeded"}),
                ));
            }
            let dir = layout.chat_channel_dir(&dir_name);
            fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
            let path = dir.join(format!("{}.json", message.id));
            if !path.exists() {
                fs::write(&path, serde_json::to_vec_pretty(&message)?)
                    .with_context(|| format!("write {}", path.display()))?;
                ledger.accepted_bytes += size;
                save_chat_ledger(&layout, &dir_name, &ledger)?;
            }
            Ok(PeerMessage::response_to(
                &request,
                "chat.channel.deliver.response",
                json!({"status": "accepted", "id": message.id}),
            ))
        }
        "chat.channel.fetch" => {
            let channel = request
                .payload
                .get("channel")
                .and_then(|value| value.as_str())
                .context("missing channel")?;
            validate_channel(channel)?;
            let dir_name = channel_dir_name(channel)?;
            let since = request
                .payload
                .get("since")
                .and_then(|value| value.as_str())
                .and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
                .map(|dt| dt.with_timezone(&chrono::Utc));
            let dir = layout.chat_channel_dir(&dir_name);
            let mut messages = Vec::new();
            if dir.exists() {
                for entry in
                    fs::read_dir(&dir).with_context(|| format!("read {}", dir.display()))?
                {
                    let entry = entry?;
                    if entry.path().extension().and_then(|ext| ext.to_str()) != Some("json") {
                        continue;
                    }
                    let bytes = fs::read(entry.path())?;
                    if let Ok(message) = serde_json::from_slice::<ChatMessage>(&bytes)
                        && since.is_none_or(|cutoff| message.sent_at > cutoff)
                    {
                        messages.push(message);
                    }
                }
            }
            messages.sort_by_key(|message| message.sent_at);
            // No cursor given (e.g. a fresh `/join`): cap the response
            // instead of handing back a channel's entire history at once.
            if since.is_none() && messages.len() > 200 {
                let drop = messages.len() - 200;
                messages.drain(0..drop);
            }
            Ok(PeerMessage::response_to(
                &request,
                "chat.channel.fetch.response",
                json!({"messages": messages}),
            ))
        }
        "board.post.deliver" => {
            let post: Post = serde_json::from_value(
                request
                    .payload
                    .get("post")
                    .context("missing post")?
                    .clone(),
            )?;
            if !post.verify_signature()? {
                anyhow::bail!("board post signature invalid");
            }
            validate_board(&post.board)?;
            let dir_name = board_dir_name(&post.board)?;
            let size = post.approx_size();
            let mut ledger = load_board_ledger(&layout, &dir_name)?.unwrap_or_default();
            if ledger.accepted_bytes + size > rymnet_account::quota_bytes(board.quota_mb) {
                return Ok(PeerMessage::response_to(
                    &request,
                    "board.post.deliver.response",
                    json!({"status": "quota_exceeded"}),
                ));
            }
            let posts_dir = layout.board_posts_dir(&dir_name);
            fs::create_dir_all(&posts_dir).with_context(|| format!("create {}", posts_dir.display()))?;
            let path = layout.board_post_path(&dir_name, &post.id);
            if !path.exists() {
                fs::write(&path, serde_json::to_vec_pretty(&post)?)
                    .with_context(|| format!("write {}", path.display()))?;
                ledger.accepted_bytes += size;
                save_board_ledger(&layout, &dir_name, &ledger)?;
                append_board_index(&layout, &dir_name, &post.summary())?;
            }
            Ok(PeerMessage::response_to(
                &request,
                "board.post.deliver.response",
                json!({"status": "accepted", "id": post.id}),
            ))
        }
        "board.post.fetch" => {
            let board_name = request
                .payload
                .get("board")
                .and_then(|value| value.as_str())
                .context("missing board")?;
            validate_board(board_name)?;
            let dir_name = board_dir_name(board_name)?;
            let since = request
                .payload
                .get("since")
                .and_then(|value| value.as_str())
                .and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
                .map(|dt| dt.with_timezone(&chrono::Utc));
            let posts = read_board_index(&layout, &dir_name, since);
            Ok(PeerMessage::response_to(
                &request,
                "board.post.fetch.response",
                json!({"posts": posts}),
            ))
        }
        "board.post.get" => {
            let board_name = request
                .payload
                .get("board")
                .and_then(|value| value.as_str())
                .context("missing board")?;
            validate_board(board_name)?;
            let dir_name = board_dir_name(board_name)?;
            let post_id = request
                .payload
                .get("post_id")
                .and_then(|value| value.as_str())
                .context("missing post_id")?;
            let path = layout.board_post_path(&dir_name, post_id);
            let post = if path.exists() {
                let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
                Some(serde_json::from_slice::<Post>(&bytes)?)
            } else {
                None
            };
            Ok(PeerMessage::response_to(
                &request,
                "board.post.get.response",
                json!({"post": post}),
            ))
        }
        "board.comment.deliver" => {
            let comment: Comment = serde_json::from_value(
                request
                    .payload
                    .get("comment")
                    .context("missing comment")?
                    .clone(),
            )?;
            if !comment.verify_signature()? {
                anyhow::bail!("board comment signature invalid");
            }
            validate_board(&comment.board)?;
            let dir_name = board_dir_name(&comment.board)?;
            let size = comment.approx_size();
            let mut ledger = load_board_ledger(&layout, &dir_name)?.unwrap_or_default();
            if ledger.accepted_bytes + size > rymnet_account::quota_bytes(board.quota_mb) {
                return Ok(PeerMessage::response_to(
                    &request,
                    "board.comment.deliver.response",
                    json!({"status": "quota_exceeded"}),
                ));
            }
            let dir = layout.board_comments_dir(&dir_name, &comment.post_id);
            fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
            let path = dir.join(format!("{}.json", comment.id));
            if !path.exists() {
                fs::write(&path, serde_json::to_vec_pretty(&comment)?)
                    .with_context(|| format!("write {}", path.display()))?;
                ledger.accepted_bytes += size;
                save_board_ledger(&layout, &dir_name, &ledger)?;
            }
            Ok(PeerMessage::response_to(
                &request,
                "board.comment.deliver.response",
                json!({"status": "accepted", "id": comment.id}),
            ))
        }
        "board.comment.fetch" => {
            let board_name = request
                .payload
                .get("board")
                .and_then(|value| value.as_str())
                .context("missing board")?;
            validate_board(board_name)?;
            let dir_name = board_dir_name(board_name)?;
            let post_id = request
                .payload
                .get("post_id")
                .and_then(|value| value.as_str())
                .context("missing post_id")?;
            let dir = layout.board_comments_dir(&dir_name, post_id);
            let mut comments = Vec::new();
            if dir.exists() {
                for entry in
                    fs::read_dir(&dir).with_context(|| format!("read {}", dir.display()))?
                {
                    let entry = entry?;
                    if entry.path().extension().and_then(|ext| ext.to_str()) != Some("json") {
                        continue;
                    }
                    let bytes = fs::read(entry.path())?;
                    if let Ok(comment) = serde_json::from_slice::<Comment>(&bytes) {
                        comments.push(comment);
                    }
                }
            }
            comments.sort_by_key(|comment| comment.created_at);
            Ok(PeerMessage::response_to(
                &request,
                "board.comment.fetch.response",
                json!({"comments": comments}),
            ))
        }
        "file.chunk.offer" => {
            let to = request
                .payload
                .get("to")
                .and_then(|value| value.as_str())
                .context("missing to")?;
            // `to` becomes a filename component in the ledger path below --
            // reject anything that isn't a well-formed username before it
            // ever reaches the filesystem, the same shape check account
            // claims already enforce at claim time.
            rymnet_account::validate_username(to)?;
            let hash = request
                .payload
                .get("hash")
                .and_then(|value| value.as_str())
                .context("missing hash")?;
            let bytes = b64_to_bytes(
                request
                    .payload
                    .get("bytes")
                    .and_then(|value| value.as_str())
                    .context("missing bytes")?,
            )?;
            if !ChunkStore::new(layout.chunks_root()).verify(hash, &bytes) {
                anyhow::bail!("file chunk hash verification failed");
            }
            let max_bytes = files.max_file_size_mb.saturating_mul(1024 * 1024);
            if bytes.len() as u64 > max_bytes {
                return Ok(PeerMessage::response_to(
                    &request,
                    "file.chunk.offer.response",
                    json!({"status": "too_large"}),
                ));
            }
            let size = bytes.len() as u64;
            let mut ledger = load_file_ledger(&layout, to)?.unwrap_or_default();
            if ledger.accepted_bytes + size > rymnet_account::quota_bytes(files.quota_mb) {
                return Ok(PeerMessage::response_to(
                    &request,
                    "file.chunk.offer.response",
                    json!({"status": "quota_exceeded"}),
                ));
            }
            let store = ChunkStore::new(layout.chunks_root());
            if !store.exists(hash)? {
                store.put(&bytes)?;
                ledger.accepted_bytes += size;
                save_file_ledger(&layout, to, &ledger)?;
            }
            Ok(PeerMessage::response_to(
                &request,
                "file.chunk.offer.response",
                json!({"status": "accepted", "hash": hash}),
            ))
        }
        "file.chunk.request" => {
            let hash = request
                .payload
                .get("hash")
                .and_then(|value| value.as_str())
                .context("missing hash")?;
            let store = ChunkStore::new(layout.chunks_root());
            let bytes = store.get(hash)?;
            if !store.verify(hash, &bytes) {
                anyhow::bail!("file chunk hash verification failed");
            }
            Ok(PeerMessage::response_to(
                &request,
                "file.chunk.response",
                json!({"hash": hash, "bytes": bytes_to_b64(&bytes)}),
            ))
        }
        _ => Ok(PeerMessage::response_to(
            &request,
            "peer.error",
            json!({"error": format!("unsupported message type {}", request.message_type)}),
        )),
    }
}

/// Tracks how many ciphertext bytes this node is currently hosting for a
/// given username, so `account.chunk.offer` can enforce the per-account
/// quota even though nothing about the plaintext content is visible to
/// us. `chunk_hashes` exists purely so periodic resync can re-offer a
/// user's vfs chunks to other peers later -- `find_pointer` alone can't
/// tell us which hashes belong to a user, since the manifest that lists
/// them is encrypted with a key this node never has.
#[derive(Debug, Serialize, Deserialize)]
struct HostedLedger {
    vfs_version: u64,
    accepted_bytes: u64,
    #[serde(default)]
    chunk_hashes: std::collections::HashSet<String>,
}

fn hosted_ledger_path(layout: &DataLayout, username: &str) -> std::path::PathBuf {
    layout.account_hosted_dir().join(format!("{username}.json"))
}

fn load_hosted_ledger(layout: &DataLayout, username: &str) -> Result<Option<HostedLedger>> {
    let path = hosted_ledger_path(layout, username);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    Ok(Some(serde_json::from_slice(&bytes)?))
}

fn save_hosted_ledger(layout: &DataLayout, username: &str, ledger: &HostedLedger) -> Result<()> {
    let path = hosted_ledger_path(layout, username);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(&path, serde_json::to_vec_pretty(ledger)?)
        .with_context(|| format!("write {}", path.display()))
}

/// Cumulative ciphertext bytes this node is relaying in `username`'s
/// mailbox. Unlike the vfs ledger, mail has no "latest version replaces
/// old" model -- every delivered message adds to the total until the
/// recipient's own client cleans things up (not yet implemented; see
/// BUILD_OVERVIEW.md).
#[derive(Debug, Default, Serialize, Deserialize)]
struct MailHostedLedger {
    accepted_bytes: u64,
}

fn mail_ledger_path(layout: &DataLayout, username: &str) -> std::path::PathBuf {
    layout.mail_hosted_dir().join(format!("{username}.json"))
}

fn load_mail_ledger(layout: &DataLayout, username: &str) -> Result<Option<MailHostedLedger>> {
    let path = mail_ledger_path(layout, username);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    Ok(Some(serde_json::from_slice(&bytes)?))
}

fn save_mail_ledger(layout: &DataLayout, username: &str, ledger: &MailHostedLedger) -> Result<()> {
    let path = mail_ledger_path(layout, username);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(&path, serde_json::to_vec_pretty(ledger)?)
        .with_context(|| format!("write {}", path.display()))
}

/// Cumulative body bytes this node is relaying for a channel. Same
/// "every message adds to the total, nothing prunes it yet" model as
/// `MailHostedLedger`.
#[derive(Debug, Default, Serialize, Deserialize)]
struct ChatHostedLedger {
    accepted_bytes: u64,
}

fn chat_ledger_path(layout: &DataLayout, channel_dir_name: &str) -> std::path::PathBuf {
    layout.chat_hosted_dir().join(format!("{channel_dir_name}.json"))
}

fn load_chat_ledger(layout: &DataLayout, channel_dir_name: &str) -> Result<Option<ChatHostedLedger>> {
    let path = chat_ledger_path(layout, channel_dir_name);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    Ok(Some(serde_json::from_slice(&bytes)?))
}

fn save_chat_ledger(layout: &DataLayout, channel_dir_name: &str, ledger: &ChatHostedLedger) -> Result<()> {
    let path = chat_ledger_path(layout, channel_dir_name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(&path, serde_json::to_vec_pretty(ledger)?)
        .with_context(|| format!("write {}", path.display()))
}

/// Cumulative post + comment body bytes this node is relaying for a
/// board. Same "every record adds to the total, nothing prunes it yet"
/// model as `ChatHostedLedger`.
#[derive(Debug, Default, Serialize, Deserialize)]
struct BoardHostedLedger {
    accepted_bytes: u64,
}

fn board_ledger_path(layout: &DataLayout, board_dir_name: &str) -> std::path::PathBuf {
    layout.board_hosted_dir().join(format!("{board_dir_name}.json"))
}

fn load_board_ledger(layout: &DataLayout, board_dir_name: &str) -> Result<Option<BoardHostedLedger>> {
    let path = board_ledger_path(layout, board_dir_name);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    Ok(Some(serde_json::from_slice(&bytes)?))
}

fn save_board_ledger(layout: &DataLayout, board_dir_name: &str, ledger: &BoardHostedLedger) -> Result<()> {
    let path = board_ledger_path(layout, board_dir_name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(&path, serde_json::to_vec_pretty(ledger)?)
        .with_context(|| format!("write {}", path.display()))
}

/// Appends one compact `PostSummary` line to a board's index -- never a
/// read-modify-write of the whole file. This is the one deliberate
/// storage-shape difference from chat (which has no index at all): it's
/// what keeps `board.post.fetch` cheap at thousands of posts instead of
/// requiring a full directory scan of every post body on every list
/// request. See `PostSummary`'s own doc comment for the full reasoning.
fn append_board_index(layout: &DataLayout, board_dir_name: &str, summary: &PostSummary) -> Result<()> {
    use std::io::Write;
    let path = layout.board_index_path(board_dir_name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let mut line = serde_json::to_string(summary)?;
    line.push('\n');
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    file.write_all(line.as_bytes())
        .with_context(|| format!("append {}", path.display()))
}

/// Reads a board's compact index (never the full posts directory --
/// what keeps this cheap at thousands of posts, see `PostSummary`'s own
/// doc comment), optionally filtered to entries newer than `since`, and
/// capped at the newest 200 when no cursor is given -- mirrors
/// `chat.channel.fetch`'s own cap exactly.
fn read_board_index(layout: &DataLayout, board_dir_name: &str, since: Option<chrono::DateTime<chrono::Utc>>) -> Vec<PostSummary> {
    let index_path = layout.board_index_path(board_dir_name);
    let mut posts = Vec::new();
    if let Ok(text) = fs::read_to_string(&index_path) {
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(summary) = serde_json::from_str::<PostSummary>(line)
                && since.is_none_or(|cutoff| summary.created_at > cutoff)
            {
                posts.push(summary);
            }
        }
    }
    posts.sort_by_key(|post| post.created_at);
    if since.is_none() && posts.len() > 200 {
        let drop = posts.len() - 200;
        posts.drain(0..drop);
    }
    posts
}

#[cfg(test)]
mod board_index_tests {
    use super::*;
    use rymnet_core::NodeIdentity;

    fn scratch_layout() -> DataLayout {
        let dir = std::env::temp_dir().join(format!(
            "rymnetd-board-index-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
        DataLayout::from_root(dir)
    }

    #[test]
    fn append_is_a_pure_append_never_a_read_modify_write() {
        let layout = scratch_layout();
        let identity = NodeIdentity::generate();
        for i in 0..5 {
            let post = Post::create("rust", "alice", &identity, &format!("post {i}"), "body", None).unwrap();
            append_board_index(&layout, "rust", &post.summary()).unwrap();
        }
        let text = fs::read_to_string(layout.board_index_path("rust")).unwrap();
        assert_eq!(text.lines().count(), 5, "one line per appended summary, nothing rewritten");

        let posts = read_board_index(&layout, "rust", None);
        assert_eq!(posts.len(), 5);
        let _ = fs::remove_dir_all(&layout.root);
    }

    #[test]
    fn since_cursor_returns_only_newer_entries() {
        let layout = scratch_layout();
        let identity = NodeIdentity::generate();

        let first = Post::create("rust", "alice", &identity, "first", "body", None).unwrap();
        append_board_index(&layout, "rust", &first.summary()).unwrap();

        // A cursor set to the first post's own timestamp should exclude
        // it (strictly-greater-than, matching `chat.channel.fetch`'s own
        // semantics) and include anything appended after.
        let cursor = first.created_at;
        std::thread::sleep(std::time::Duration::from_millis(2));
        let second = Post::create("rust", "bob", &identity, "second", "body", None).unwrap();
        append_board_index(&layout, "rust", &second.summary()).unwrap();

        let since_first = read_board_index(&layout, "rust", Some(cursor));
        assert_eq!(since_first.len(), 1);
        assert_eq!(since_first[0].id, second.id);

        let all = read_board_index(&layout, "rust", None);
        assert_eq!(all.len(), 2);

        let _ = fs::remove_dir_all(&layout.root);
    }

    #[test]
    fn no_cursor_caps_at_the_newest_two_hundred() {
        let layout = scratch_layout();
        let identity = NodeIdentity::generate();
        for i in 0..205 {
            let post = Post::create("rust", "alice", &identity, &format!("post {i}"), "body", None).unwrap();
            append_board_index(&layout, "rust", &post.summary()).unwrap();
        }
        let posts = read_board_index(&layout, "rust", None);
        assert_eq!(posts.len(), 200, "uncapped requests should still be bounded to the newest 200");
        let _ = fs::remove_dir_all(&layout.root);
    }
}

/// Cumulative sealed-ciphertext bytes this node is relaying for `to`'s
/// file transfers. Same "every offer adds to the total, nothing prunes
/// yet" model as `MailHostedLedger`/`ChatHostedLedger` -- the bytes are
/// opaque to this node either way (sealed, not signed-plaintext), so
/// there's nothing more this ledger could usefully track.
#[derive(Debug, Default, Serialize, Deserialize)]
struct FileHostedLedger {
    accepted_bytes: u64,
}

fn file_ledger_path(layout: &DataLayout, to: &str) -> std::path::PathBuf {
    layout.files_hosted_dir().join(format!("{to}.json"))
}

fn load_file_ledger(layout: &DataLayout, to: &str) -> Result<Option<FileHostedLedger>> {
    let path = file_ledger_path(layout, to);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    Ok(Some(serde_json::from_slice(&bytes)?))
}

fn save_file_ledger(layout: &DataLayout, to: &str, ledger: &FileHostedLedger) -> Result<()> {
    let path = file_ledger_path(layout, to);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(&path, serde_json::to_vec_pretty(ledger)?)
        .with_context(|| format!("write {}", path.display()))
}

fn search_record_matches(record: &SearchRecord, query: &str) -> bool {
    let haystack = format!(
        "{} {} {} {} {} {}",
        record.domain,
        record.path,
        record.title,
        record.description,
        record.headings.join(" "),
        record.keywords.join(" ")
    )
    .to_ascii_lowercase();
    query.split_whitespace().all(|term| haystack.contains(term))
}

fn accept_replica_offer(layout: &DataLayout, payload: &serde_json::Value) -> Result<()> {
    let claim: DomainClaimRecord =
        serde_json::from_value(payload.get("claim").context("missing claim")?.clone())?;
    if !claim.verify()? || claim.is_expired() {
        anyhow::bail!("invalid replica claim");
    }
    let manifest: SiteManifest =
        serde_json::from_value(payload.get("manifest").context("missing manifest")?.clone())?;
    if manifest.domain != claim.domain {
        anyhow::bail!("replica manifest domain mismatch");
    }
    if !manifest.verify(&claim.owner_public_key)? {
        anyhow::bail!("replica manifest signature failed");
    }

    let store = ChunkStore::new(layout.chunks_root());
    let chunks = payload
        .get("chunks")
        .and_then(|value| value.as_array())
        .context("missing chunks")?;
    for chunk in chunks {
        let hash = chunk
            .get("hash")
            .and_then(|value| value.as_str())
            .context("missing chunk hash")?;
        let bytes = b64_to_bytes(
            chunk
                .get("bytes")
                .and_then(|value| value.as_str())
                .context("missing chunk bytes")?,
        )?;
        if !store.verify(hash, &bytes) {
            anyhow::bail!("replica chunk hash failed: {hash}");
        }
        store.put(&bytes)?;
    }

    for file in &manifest.files {
        if !store.exists(&file.hash)? {
            anyhow::bail!("replica offer missing chunk {}", file.hash);
        }
    }

    claim.save(layout.domain_claims_dir())?;
    save_replicated_manifest(layout.site_root(), &manifest)?;
    set_pin(layout.site_root(), &manifest.domain, true)?;
    Ok(())
}
