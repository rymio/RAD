use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use rymnet_core::{NodeIdentity, node::public_key_display};
use rymnet_domain::{DomainClaimRecord, claim_domain, find_claim, list_claims, validate_domain};
use rymnet_gateway::gateway_url;
use rymnet_network::{PeerMessage, b64_to_bytes, bytes_to_b64};
use rymnet_search::{
    SearchRecord, clear_index, create_search_records_for_manifest, default_index_root,
    default_records_root, index_records, index_site_by_domain, index_status, list_search_records,
    rebuild_index, save_verified_search_record, search,
};
use rymnet_site::{
    SiteManifest, SiteValidation, create_site, is_pinned, load_latest_manifest, publish_site,
    save_replicated_manifest, set_pin, upload_site, validate_site_source,
};
use rymnet_storage::{AppConfig, ChunkStore, DataLayout, PeerConfig, PeerEntry};
use serde_json::json;
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(name = "rym", version, about = "Rymnet command-line client")]
struct Cli {
    #[arg(long, global = true, value_name = "DIR")]
    data_dir: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Version,
    Init,
    Node {
        #[command(subcommand)]
        command: NodeCommand,
    },
    Domain {
        #[command(subcommand)]
        command: DomainCommand,
    },
    Peer {
        #[command(subcommand)]
        command: PeerCommand,
    },
    Site {
        #[command(subcommand)]
        command: SiteCommand,
    },
    Search {
        #[arg(long)]
        local: bool,
        #[arg(long)]
        network: bool,
        #[arg(long)]
        domain: Option<String>,
        query: Vec<String>,
    },
    Index {
        #[command(subcommand)]
        command: IndexCommand,
    },
    Open {
        domain: String,
    },
    Time,
}

#[derive(Debug, Subcommand)]
enum NodeCommand {
    Start,
    Stop,
    Status,
    Id,
    Peers,
    Address,
}

#[derive(Debug, Subcommand)]
enum DomainCommand {
    Search { domain: String },
    Claim { domain: String },
    List,
    Info { domain: String },
    Renew { domain: String },
    Release { domain: String },
}

#[derive(Debug, Subcommand)]
enum PeerCommand {
    Add {
        name: String,
        public_key: String,
        endpoint: String,
    },
    Remove {
        name: String,
    },
    List,
    Ping {
        name: String,
    },
    Connect {
        /// Host, or host:port (defaults to the peer port 7444)
        address: String,
        #[arg(long)]
        name: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum SiteCommand {
    Create {
        domain: String,
    },
    Upload {
        domain: String,
        folder: PathBuf,
    },
    Validate {
        domain: String,
    },
    Publish {
        domain: String,
    },
    Status {
        domain: String,
    },
    Versions {
        domain: String,
    },
    Rollback {
        domain: String,
        #[arg(long)]
        version: u64,
    },
    Replicate {
        domain: String,
        #[arg(long, default_value_t = 3)]
        copies: u16,
    },
    Fetch {
        domain: String,
    },
    Pin {
        domain: String,
    },
    Unpin {
        domain: String,
    },
    Replicas {
        domain: String,
    },
}

#[derive(Debug, Subcommand)]
enum IndexCommand {
    Status,
    Rebuild,
    Site { domain: String },
    Clear,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_target(false)
        .init();

    let cli = Cli::parse();
    let layout = layout(cli.data_dir.as_deref())?;

    match cli.command {
        Commands::Version => println!("rym {}", env!("CARGO_PKG_VERSION")),
        Commands::Init => init(&layout)?,
        Commands::Node { command } => node(command, &layout)?,
        Commands::Domain { command } => domain(command, &layout)?,
        Commands::Peer { command } => peer(command, &layout).await?,
        Commands::Site { command } => site(command, &layout).await?,
        Commands::Search {
            local: _,
            network,
            domain,
            query,
        } => search_command(&layout, query, domain.as_deref(), network).await?,
        Commands::Index { command } => index(command, &layout)?,
        Commands::Open { domain } => {
            validate_domain(&domain)?;
            println!("{}", gateway_url(&domain));
        }
        Commands::Time => time_command(&layout).await?,
    }

    Ok(())
}

fn layout(data_dir: Option<&str>) -> Result<DataLayout> {
    Ok(match data_dir {
        Some(dir) => DataLayout::from_root(rymnet_storage::layout::expand_tilde(dir)?),
        None => DataLayout::default()?,
    })
}

fn init(layout: &DataLayout) -> Result<()> {
    layout.ensure_all()?;
    AppConfig::default().save_new(layout.config_path())?;

    if layout.identity_path().exists() {
        let identity = NodeIdentity::load(layout.identity_path())?;
        println!("node identity already exists");
        println!("node id: {}", identity.node_id()?);
        println!("config: {}", layout.config_path().display());
        return Ok(());
    }

    let identity = NodeIdentity::generate();
    identity.save_new(layout.identity_path())?;

    println!("node identity created");
    println!("node id: {}", identity.node_id()?);
    println!("config created: {}", layout.config_path().display());
    println!("peers file: {}", layout.peers_path().display());
    Ok(())
}

fn node(command: NodeCommand, layout: &DataLayout) -> Result<()> {
    match command {
        NodeCommand::Id => {
            let identity = load_identity(layout)?;
            println!("{}", identity.node_id()?);
        }
        NodeCommand::Status => {
            let identity = load_identity(layout)?;
            println!("Node: {}", identity.node_id()?);
            println!("Data: {}", layout.root.display());
            println!(
                "Gateway: {}",
                gateway_url("<domain.rym>").replace("/<domain.rym>/", "")
            );
        }
        NodeCommand::Address => {
            let config = AppConfig::load(layout.config_path())?;
            println!("{}", config.node.listen_addr);
        }
        NodeCommand::Peers => {
            println!("{}", layout.peers_path().display());
        }
        NodeCommand::Start => {
            println!("Run rymnetd run to start the daemon in the foreground.");
        }
        NodeCommand::Stop => {
            println!("Daemon stop via admin API is not implemented in Milestone 1.");
        }
    }
    Ok(())
}

fn domain(command: DomainCommand, layout: &DataLayout) -> Result<()> {
    match command {
        DomainCommand::Search { domain } => {
            validate_domain(&domain)?;
            match find_claim(layout.domain_claims_dir(), &domain)? {
                Some(claim) if claim.verify()? && !claim.is_expired() => {
                    println!("{domain} is already claimed");
                    println!(
                        "Owner: {}",
                        public_key_display(&base64_decode(&claim.owner_public_key)?)
                    );
                    println!("Expires: {}", claim.expires_at.date_naive());
                }
                _ => println!("{domain} is available"),
            }
        }
        DomainCommand::Claim { domain } => {
            let identity = load_identity(layout)?;
            let claim = claim_domain(layout.domain_claims_dir(), &domain, &identity)?;
            println!("Domain claimed: {}", claim.domain);
            println!(
                "Owner key: {}",
                public_key_display(&base64_decode(&claim.owner_public_key)?)
            );
            println!("Status: saved locally; peer announcement pending networking milestone");
        }
        DomainCommand::List => {
            let claims = list_claims(layout.domain_claims_dir())?;
            if claims.is_empty() {
                println!("No local domain claims.");
            } else {
                for claim in claims {
                    let status = if claim.verify()? && !claim.is_expired() {
                        "valid"
                    } else {
                        "invalid-or-expired"
                    };
                    println!(
                        "{}  {}  expires {}",
                        claim.domain,
                        status,
                        claim.expires_at.date_naive()
                    );
                }
            }
        }
        DomainCommand::Info { domain } => {
            validate_domain(&domain)?;
            let claim = find_claim(layout.domain_claims_dir(), &domain)?
                .with_context(|| format!("no local claim for {domain}"))?;
            println!("Domain: {}", claim.domain);
            println!(
                "Owner: {}",
                public_key_display(&base64_decode(&claim.owner_public_key)?)
            );
            println!("Created: {}", claim.created_at);
            println!("Expires: {}", claim.expires_at);
            println!("Signature valid: {}", claim.verify()?);
            println!("Hash: {}", claim.hash()?);
        }
        DomainCommand::Renew { domain } => {
            validate_domain(&domain)?;
            println!("Domain renew is not implemented in Milestone 2: {domain}");
        }
        DomainCommand::Release { domain } => {
            validate_domain(&domain)?;
            println!("Domain release is not implemented in Milestone 2: {domain}");
        }
    }
    Ok(())
}

async fn peer(command: PeerCommand, layout: &DataLayout) -> Result<()> {
    match command {
        PeerCommand::Add {
            name,
            public_key,
            endpoint,
        } => {
            let mut config = PeerConfig::load(layout.peers_path())?;
            config.add(PeerEntry {
                name: name.clone(),
                public_key,
                endpoint,
            });
            config.save(layout.peers_path())?;
            println!("Peer added: {name}");
        }
        PeerCommand::Remove { name } => {
            let mut config = PeerConfig::load(layout.peers_path())?;
            if config.remove(&name) {
                config.save(layout.peers_path())?;
                println!("Peer removed: {name}");
            } else {
                println!("Peer not found: {name}");
            }
        }
        PeerCommand::List => {
            let config = PeerConfig::load(layout.peers_path())?;
            if config.peers.is_empty() {
                println!("No peers configured.");
            } else {
                for peer in config.peers {
                    println!("{}  {}  {}", peer.name, peer.public_key, peer.endpoint);
                }
            }
        }
        PeerCommand::Ping { name } => {
            let identity = load_identity(layout)?;
            let config = PeerConfig::load(layout.peers_path())?;
            let peer = config
                .find(&name)
                .with_context(|| format!("unknown peer {name}"))?;
            let response = rymnet_network::ping(peer, &identity).await?;
            println!("Peer {name}: {}", response.payload);
        }
        PeerCommand::Connect { address, name } => connect_peer(layout, &address, name).await?,
    }
    Ok(())
}

/// Rymnet has no central time authority, so "network time" here means an
/// NTP-style estimate: sync against every configured peer, then take the
/// median offset (median rather than mean so one slow or lying peer can't
/// skew the result).
async fn time_command(layout: &DataLayout) -> Result<()> {
    let identity = load_identity(layout)?;
    let config = PeerConfig::load(layout.peers_path())?;
    let local_now = rymnet_network::unix_millis_now();
    println!("Local time:  {}", format_millis(local_now));

    if config.peers.is_empty() {
        println!("No peers configured; nothing to sync against (see `rym peer add`).");
        return Ok(());
    }

    let mut offsets = Vec::new();
    for peer in &config.peers {
        match rymnet_network::time_sync(peer, &identity).await {
            Ok(sample) => {
                println!(
                    "{:<20} peer time {}  offset {:+}ms  rtt {}ms",
                    peer.name,
                    format_millis(sample.peer_time_ms),
                    sample.offset_ms,
                    sample.round_trip_ms,
                );
                offsets.push(sample.offset_ms);
            }
            Err(err) => println!("{:<20} unreachable: {err:#}", peer.name),
        }
    }

    if offsets.is_empty() {
        println!("No peers reachable; cannot compute network time.");
        return Ok(());
    }
    offsets.sort_unstable();
    let median_offset = offsets[offsets.len() / 2];
    println!();
    println!(
        "Network time (median of {} peer(s)): {}",
        offsets.len(),
        format_millis(local_now + median_offset)
    );
    println!("Clock offset from local system time: {median_offset:+}ms");
    Ok(())
}

fn format_millis(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| ms.to_string())
}

/// Bootstraps into a network from a single known address: connects
/// without needing the remote's public key in advance (the handshake
/// itself reveals it), adds that node as a peer, then asks it for its own
/// known peers and independently pings each one before trusting it --
/// turning "I know one node's IP" into "I'm peered with its whole known
/// mesh" in one step, without ever taking the remote's word for another
/// peer's identity.
async fn connect_peer(layout: &DataLayout, address: &str, name: Option<String>) -> Result<()> {
    let identity = load_identity(layout)?;
    let endpoint = normalize_endpoint(address);
    println!("Connecting to {endpoint}...");

    let (hello, response) = rymnet_network::send_request_discover(
        &endpoint,
        &identity,
        PeerMessage::new("peer.list.request", json!({})),
    )
    .await
    .with_context(|| format!("connect to {endpoint}"))?;

    if hello.public_key == identity.public_key {
        anyhow::bail!(
            "that address is this node's own identity; refusing to add yourself as a peer"
        );
    }

    let mut config = PeerConfig::load(layout.peers_path())?;
    let peer_name = name.unwrap_or_else(|| default_peer_name(&endpoint));
    config.add(PeerEntry {
        name: peer_name.clone(),
        public_key: hello.public_key.clone(),
        endpoint: endpoint.clone(),
    });
    config.save(layout.peers_path())?;
    println!("Connected: {peer_name} ({})", hello.node_id);

    let discovered = response
        .payload
        .get("peers")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    if discovered.is_empty() {
        if response
            .payload
            .get("shared")
            .and_then(|value| value.as_bool())
            == Some(false)
        {
            println!(
                "{peer_name} does not share its peer list (share_peer_list = false on that node)."
            );
        }
        return Ok(());
    }

    println!(
        "{peer_name} knows about {} other peer(s); verifying each before adding...",
        discovered.len()
    );
    let mut added = 0;
    let mut skipped = 0;
    for value in discovered {
        let Ok(candidate) = serde_json::from_value::<PeerEntry>(value) else {
            skipped += 1;
            continue;
        };
        if candidate.public_key == identity.public_key || candidate.public_key == hello.public_key {
            continue; // that's us, or the node we just connected to
        }
        if config
            .peers
            .iter()
            .any(|existing| existing.public_key == candidate.public_key)
        {
            continue; // already have them, possibly under a different name
        }

        match rymnet_network::ping(&candidate, &identity).await {
            Ok(_) => {
                let mut candidate_name = candidate.name.clone();
                if config.find(&candidate_name).is_some() {
                    candidate_name =
                        format!("{candidate_name}-{}", short_key(&candidate.public_key));
                }
                config.add(PeerEntry {
                    name: candidate_name.clone(),
                    public_key: candidate.public_key.clone(),
                    endpoint: candidate.endpoint.clone(),
                });
                println!("  + added {candidate_name} ({})", candidate.endpoint);
                added += 1;
            }
            Err(err) => {
                println!(
                    "  - skipped {} ({}): unreachable or key mismatch ({err:#})",
                    candidate.name, candidate.endpoint
                );
                skipped += 1;
            }
        }
    }
    config.save(layout.peers_path())?;
    println!("Done: added {added} new peer(s), skipped {skipped}.");
    Ok(())
}

fn normalize_endpoint(address: &str) -> String {
    if address.contains(':') {
        address.to_string()
    } else {
        format!("{address}:7444")
    }
}

fn default_peer_name(endpoint: &str) -> String {
    endpoint.split(':').next().unwrap_or(endpoint).to_string()
}

fn short_key(public_key: &str) -> String {
    public_key.chars().take(6).collect()
}

async fn site(command: SiteCommand, layout: &DataLayout) -> Result<()> {
    match command {
        SiteCommand::Create { domain } => {
            validate_domain(&domain)?;
            ensure_own_domain(layout, &domain)?;
            let source = create_site(&domain, layout.site_root())?;
            println!("Site created: {domain}");
            println!("Source: {}", source.display());
        }
        SiteCommand::Upload { domain, folder } => {
            validate_domain(&domain)?;
            ensure_own_domain(layout, &domain)?;
            let validation = upload_site(&domain, layout.site_root(), &folder)
                .with_context(|| format!("upload site from {}", folder.display()))?;
            print_validation(&validation);
            if validation.is_ok() {
                println!("Upload accepted: {domain}");
            } else {
                println!("Upload rejected: {domain}");
            }
        }
        SiteCommand::Validate { domain } => {
            validate_domain(&domain)?;
            let validation = validate_site_source(&domain, layout.site_root())?;
            print_validation(&validation);
            if validation.is_ok() {
                println!("Site valid: {domain}");
            } else {
                println!("Site invalid: {domain}");
            }
        }
        SiteCommand::Publish { domain } => {
            validate_domain(&domain)?;
            let identity = ensure_own_domain(layout, &domain)?;
            let result =
                publish_site(&domain, layout.site_root(), layout.chunks_root(), &identity)?;
            println!("Site published: {domain}");
            println!("Version: {}", result.manifest.site_version);
            println!("Files hashed: {}", result.files_hashed);
            println!("Manifest: {}", result.manifest_hash);
            println!("Manifest saved: {}", result.manifest_path.display());
            println!(
                "Signature valid: {}",
                result.manifest.verify(&identity.public_key)?
            );
            let indexed = rymnet_search::index_site(
                default_index_root(layout.search_root()),
                layout.chunks_root(),
                &result.manifest,
            )?;
            println!("Indexed pages: {indexed}");
            let records = create_search_records_for_manifest(
                default_records_root(layout.search_root()),
                layout.chunks_root(),
                &result.manifest,
                &identity,
            )?;
            let record_indexed = index_records(default_index_root(layout.search_root()), &records)?;
            let announced = announce_search_records(layout, &domain, &records, &identity).await?;
            println!("Search records: {}", records.len());
            println!("Search records indexed: {record_indexed}");
            println!("Search records announced: {announced}");

            // Publishing used to only ever save locally, leaving content
            // undiscoverable via search (records were already announced
            // above) but unfetchable everywhere except this node -- an
            // easy trap, since nothing about "Site published" said the
            // content itself hadn't gone anywhere. Auto-replicate to
            // every configured peer so `publish` alone is enough.
            let peers = PeerConfig::load(layout.peers_path())?;
            if peers.peers.is_empty() {
                println!(
                    "Status: saved locally only -- no peers configured, so there's nothing to \
                     replicate to yet. Add one with `rym peer connect <ip>`, then re-run this \
                     command or `rym site replicate {domain}`."
                );
            } else {
                let replicated = replicate_site(layout, &domain, peers.peers.len() as u16).await?;
                println!("Replicated to {replicated}/{} peer(s)", peers.peers.len());
                if replicated < peers.peers.len() {
                    println!(
                        "Status: partially replicated -- some peers didn't accept it; \
                         re-run `rym site replicate {domain}` to retry"
                    );
                } else {
                    println!("Status: published and replicated to every configured peer");
                }
            }
        }
        SiteCommand::Status { domain } => {
            validate_domain(&domain)?;
            let validation = validate_site_source(&domain, layout.site_root())?;
            println!("Domain: {domain}");
            println!("Accepted files: {}", validation.accepted.len());
            println!("Rejected files: {}", validation.rejected.len());
        }
        SiteCommand::Versions { domain } => {
            validate_domain(&domain)?;
            let dir = rymnet_site::manifests_dir(layout.site_root(), &domain);
            if !dir.exists() {
                println!("No published versions for {domain}.");
            } else {
                for entry in std::fs::read_dir(&dir)? {
                    let entry = entry?;
                    if entry.path().extension().and_then(|ext| ext.to_str()) == Some("json") {
                        println!("{}", entry.path().display());
                    }
                }
            }
        }
        SiteCommand::Rollback { domain, version } => {
            validate_domain(&domain)?;
            println!("Site rollback is not implemented in Milestone 4: {domain} version {version}");
        }
        SiteCommand::Replicate { domain, copies } => {
            validate_domain(&domain)?;
            let replicated = replicate_site(layout, &domain, copies).await?;
            println!("Replication offers accepted: {replicated}/{copies}");
        }
        SiteCommand::Fetch { domain } => {
            validate_domain(&domain)?;
            let result = fetch_site(layout, &domain).await?;
            println!("Fetched site: {domain}");
            println!("From peer: {}", result.peer_name);
            println!("Manifest: {}", result.manifest_hash);
            println!("Chunks: {}", result.chunks);
        }
        SiteCommand::Pin { domain } => {
            validate_domain(&domain)?;
            set_pin(layout.site_root(), &domain, true)?;
            println!("Site pinned: {domain}");
        }
        SiteCommand::Unpin { domain } => {
            validate_domain(&domain)?;
            set_pin(layout.site_root(), &domain, false)?;
            println!("Site unpinned: {domain}");
        }
        SiteCommand::Replicas { domain } => {
            validate_domain(&domain)?;
            print_replica_status(layout, &domain)?;
        }
    }
    Ok(())
}

#[derive(Debug)]
struct FetchResult {
    peer_name: String,
    manifest_hash: String,
    chunks: usize,
}

async fn fetch_site(layout: &DataLayout, domain: &str) -> Result<FetchResult> {
    let identity = load_identity(layout)?;
    let peers = PeerConfig::load(layout.peers_path())?;
    for peer in &peers.peers {
        match fetch_site_from_peer(layout, &identity, peer, domain).await {
            Ok(result) => return Ok(result),
            Err(err) => eprintln!("Fetch from {} failed: {err:#}", peer.name),
        }
    }
    anyhow::bail!("no configured peer could provide {domain}")
}

async fn fetch_site_from_peer(
    layout: &DataLayout,
    identity: &NodeIdentity,
    peer: &PeerEntry,
    domain: &str,
) -> Result<FetchResult> {
    let response = rymnet_network::send_request(
        peer,
        identity,
        PeerMessage::new("site.manifest.request", json!({ "domain": domain })),
    )
    .await?;
    if response.message_type != "site.manifest.response" {
        anyhow::bail!("unexpected manifest response {}", response.message_type);
    }
    let claim: DomainClaimRecord = serde_json::from_value(
        response
            .payload
            .get("claim")
            .context("missing claim")?
            .clone(),
    )?;
    if claim.domain != domain || !claim.verify()? || claim.is_expired() {
        anyhow::bail!("invalid fetched claim");
    }
    let manifest: SiteManifest = serde_json::from_value(
        response
            .payload
            .get("manifest")
            .context("missing manifest")?
            .clone(),
    )?;
    if manifest.domain != domain || !manifest.verify(&claim.owner_public_key)? {
        anyhow::bail!("invalid fetched manifest");
    }
    let manifest_hash = manifest.hash()?;

    let store = ChunkStore::new(layout.chunks_root());
    let mut chunks = 0;
    for file in &manifest.files {
        if store.exists(&file.hash)? {
            continue;
        }
        let response = rymnet_network::send_request(
            peer,
            identity,
            PeerMessage::new("site.chunk.request", json!({ "hash": file.hash })),
        )
        .await?;
        if response.message_type != "site.chunk.response" {
            anyhow::bail!("unexpected chunk response {}", response.message_type);
        }
        let hash = response
            .payload
            .get("hash")
            .and_then(|value| value.as_str())
            .context("missing chunk hash")?;
        let bytes = b64_to_bytes(
            response
                .payload
                .get("bytes")
                .and_then(|value| value.as_str())
                .context("missing chunk bytes")?,
        )?;
        if hash != file.hash || !store.verify(hash, &bytes) {
            anyhow::bail!("chunk verification failed for {}", file.path);
        }
        store.put(&bytes)?;
        chunks += 1;
    }

    claim.save(layout.domain_claims_dir())?;
    save_replicated_manifest(layout.site_root(), &manifest)?;
    let indexed = rymnet_search::index_site(
        default_index_root(layout.search_root()),
        layout.chunks_root(),
        &manifest,
    )?;
    if indexed > 0 {
        println!("Indexed pages: {indexed}");
    }

    Ok(FetchResult {
        peer_name: peer.name.clone(),
        manifest_hash,
        chunks,
    })
}

async fn replicate_site(layout: &DataLayout, domain: &str, copies: u16) -> Result<usize> {
    let identity = ensure_own_domain(layout, domain)?;
    let claim = find_claim(layout.domain_claims_dir(), domain)?
        .with_context(|| format!("claim {domain} before replicating"))?;
    let manifest = load_latest_manifest(domain, layout.site_root())?
        .with_context(|| format!("publish {domain} before replicating"))?;
    if !manifest.verify(&claim.owner_public_key)? {
        anyhow::bail!("local manifest signature failed");
    }
    let store = ChunkStore::new(layout.chunks_root());
    let chunks = manifest
        .files
        .iter()
        .map(|file| {
            let bytes = store.get(&file.hash)?;
            if !store.verify(&file.hash, &bytes) {
                anyhow::bail!("chunk verification failed for {}", file.path);
            }
            Ok(json!({"hash": file.hash, "bytes": bytes_to_b64(&bytes)}))
        })
        .collect::<Result<Vec<_>>>()?;
    let payload = json!({
        "claim": claim,
        "manifest": manifest,
        "chunks": chunks,
    });

    let peers = PeerConfig::load(layout.peers_path())?;
    let mut accepted = 0;
    for peer in peers.peers.iter().take(copies as usize) {
        let response = rymnet_network::send_request(
            peer,
            &identity,
            PeerMessage::new("replica.offer", payload.clone()),
        )
        .await?;
        if response.message_type == "replica.status"
            && response
                .payload
                .get("status")
                .and_then(|value| value.as_str())
                == Some("complete")
        {
            println!("Replica complete: {}", peer.name);
            accepted += 1;
        } else {
            println!("Replica incomplete: {} {}", peer.name, response.payload);
        }
    }
    Ok(accepted)
}

fn print_replica_status(layout: &DataLayout, domain: &str) -> Result<()> {
    let manifest = load_latest_manifest(domain, layout.site_root())?
        .with_context(|| format!("no local manifest for {domain}"))?;
    let store = ChunkStore::new(layout.chunks_root());
    let complete = manifest
        .files
        .iter()
        .filter(|file| store.exists(&file.hash).unwrap_or(false))
        .count();
    println!("Domain: {domain}");
    println!("Manifest: {}", manifest.hash()?);
    println!("Pinned: {}", is_pinned(layout.site_root(), domain));
    if complete == manifest.files.len() {
        println!("Local replica: complete");
    } else {
        println!(
            "Local replica: partial, {}/{} files",
            complete,
            manifest.files.len()
        );
    }
    Ok(())
}

async fn search_command(
    layout: &DataLayout,
    query: Vec<String>,
    domain: Option<&str>,
    network: bool,
) -> Result<()> {
    if let Some(domain) = domain {
        validate_domain(domain)?;
    }
    let query = query.join(" ");
    if query.trim().is_empty() {
        anyhow::bail!("search query cannot be empty");
    }
    let mut results = search(default_index_root(layout.search_root()), &query, domain, 10)?;
    if network {
        let remote_count = search_network_records(layout, &query, domain).await?;
        if remote_count > 0 {
            results = search(default_index_root(layout.search_root()), &query, domain, 10)?;
        }
    }
    println!("Search: {query}");
    if results.is_empty() {
        println!("No results.");
    } else {
        for (index, result) in results.iter().enumerate() {
            let display_path = if result.path == "/index.html" {
                result.domain.clone()
            } else {
                format!("{}{}", result.domain, result.path)
            };
            println!();
            println!("{}. {}", index + 1, display_path);
            if !result.title.is_empty() {
                println!("   {}", result.title);
            }
            if !result.description.is_empty() {
                println!("   {}", result.description);
            }
            println!("   {}", result.url);
        }
    }
    Ok(())
}

async fn announce_search_records(
    layout: &DataLayout,
    domain: &str,
    records: &[SearchRecord],
    identity: &NodeIdentity,
) -> Result<usize> {
    if records.is_empty() {
        return Ok(0);
    }
    let claim = find_claim(layout.domain_claims_dir(), domain)?
        .with_context(|| format!("claim {domain} before announcing search records"))?;
    let peers = PeerConfig::load(layout.peers_path())?;
    let mut accepted = 0;
    for peer in &peers.peers {
        for record in records {
            let response = rymnet_network::send_request(
                peer,
                identity,
                PeerMessage::new(
                    "search.record.announce",
                    json!({"claim": &claim, "record": record}),
                ),
            )
            .await;
            match response {
                Ok(response)
                    if response.message_type == "search.record.response"
                        && response
                            .payload
                            .get("status")
                            .and_then(|value| value.as_str())
                            == Some("accepted") =>
                {
                    accepted += 1;
                }
                Ok(response) => eprintln!(
                    "Search record announce to {} returned {} {}",
                    peer.name, response.message_type, response.payload
                ),
                Err(err) => eprintln!("Search record announce to {} failed: {err:#}", peer.name),
            }
        }
    }
    Ok(accepted)
}

async fn search_network_records(
    layout: &DataLayout,
    query: &str,
    domain: Option<&str>,
) -> Result<usize> {
    let identity = load_identity(layout)?;
    let peers = PeerConfig::load(layout.peers_path())?;
    let mut accepted = 0;
    for peer in &peers.peers {
        let response = rymnet_network::send_request(
            peer,
            &identity,
            PeerMessage::new(
                "search.query",
                json!({"query": query, "domain": domain, "limit": 10}),
            ),
        )
        .await;
        let response = match response {
            Ok(response) => response,
            Err(err) => {
                eprintln!("Network search from {} failed: {err:#}", peer.name);
                continue;
            }
        };
        if response.message_type != "search.response" {
            eprintln!(
                "Network search from {} returned {}",
                peer.name, response.message_type
            );
            continue;
        }
        let items = response
            .payload
            .get("records")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let mut verified = Vec::new();
        for item in items {
            let claim: DomainClaimRecord =
                serde_json::from_value(item.get("claim").context("missing claim")?.clone())?;
            if !claim.verify()? || claim.is_expired() {
                continue;
            }
            let record: SearchRecord =
                serde_json::from_value(item.get("record").context("missing record")?.clone())?;
            if record.domain != claim.domain {
                continue;
            }
            save_verified_search_record(
                default_records_root(layout.search_root()),
                &record,
                &claim.owner_public_key,
            )?;
            claim.save(layout.domain_claims_dir())?;
            verified.push(record);
        }
        accepted += index_records(default_index_root(layout.search_root()), &verified)?;
    }
    Ok(accepted)
}

fn index(command: IndexCommand, layout: &DataLayout) -> Result<()> {
    match command {
        IndexCommand::Status => {
            let status = index_status(default_index_root(layout.search_root()))?;
            println!("Indexed documents: {}", status.documents);
            println!(
                "Search records: {}",
                list_search_records(default_records_root(layout.search_root()))?.len()
            );
        }
        IndexCommand::Rebuild => {
            let count = rebuild_index(
                default_index_root(layout.search_root()),
                layout.site_root(),
                layout.chunks_root(),
            )?;
            println!("Index rebuilt: {count} documents");
        }
        IndexCommand::Site { domain } => {
            validate_domain(&domain)?;
            let count = index_site_by_domain(
                default_index_root(layout.search_root()),
                layout.site_root(),
                layout.chunks_root(),
                &domain,
            )?;
            println!("Indexed {domain}: {count} documents");
        }
        IndexCommand::Clear => {
            clear_index(default_index_root(layout.search_root()))?;
            println!("Index cleared");
        }
    }
    Ok(())
}

fn load_identity(layout: &DataLayout) -> Result<NodeIdentity> {
    NodeIdentity::load(layout.identity_path())
        .with_context(|| "run `rym init` before using node identity commands")
}

fn ensure_own_domain(layout: &DataLayout, domain: &str) -> Result<NodeIdentity> {
    let identity = load_identity(layout)?;
    let claim = find_claim(layout.domain_claims_dir(), domain)?
        .with_context(|| format!("claim {domain} before managing its site"))?;
    if !claim.verify()? || claim.is_expired() {
        anyhow::bail!("domain claim for {domain} is invalid or expired");
    }
    if claim.owner_public_key != identity.public_key {
        anyhow::bail!("local node identity does not own {domain}");
    }
    Ok(identity)
}

fn print_validation(validation: &SiteValidation) {
    println!("Accepted files:");
    if validation.accepted.is_empty() {
        println!("  none");
    } else {
        for path in &validation.accepted {
            println!("  {path}");
        }
    }

    println!("Rejected files:");
    if validation.rejected.is_empty() {
        println!("  none");
    } else {
        for rejection in &validation.rejected {
            println!("  {} - {}", rejection.path, rejection.reason);
        }
    }
}

fn base64_decode(value: &str) -> Result<Vec<u8>> {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    STANDARD.decode(value).context("decode public key")
}
