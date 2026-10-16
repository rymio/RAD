use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppConfig {
    pub node: NodeConfig,
    pub gateway: GatewayConfig,
    pub admin: AdminConfig,
    pub site: SiteConfig,
    pub replication: ReplicationConfig,
    pub search: SearchConfig,
    #[serde(default)]
    pub account: AccountConfig,
    #[serde(default)]
    pub mail: MailConfig,
    #[serde(default)]
    pub chat: ChatConfig,
    #[serde(default)]
    pub board: BoardConfig,
    #[serde(default)]
    pub files: FileConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeConfig {
    pub data_dir: String,
    pub listen_addr: String,
    /// Whether to answer `peer.list.request` with this node's own known
    /// peers. This is what lets `rym peer connect <ip>` turn "add one
    /// node" into "join its whole known mesh" -- but it does mean
    /// revealing the names/keys/endpoints of your peers to anyone who
    /// connects and asks, not just peers you've already vetted. Set to
    /// `false` if you don't want your node to disclose that.
    #[serde(default = "default_true")]
    pub share_peer_list: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GatewayConfig {
    pub enabled: bool,
    pub listen_addr: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AdminConfig {
    pub enabled: bool,
    pub listen_addr: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SiteConfig {
    pub max_site_size_mb: u64,
    pub max_file_size_mb: u64,
    pub allow_images: bool,
    pub allow_fonts: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReplicationConfig {
    pub enabled: bool,
    pub default_copies: u16,
    pub max_storage_mb: u64,
    /// How often (in seconds) this node re-announces everything it has
    /// locally published -- domain claims, site manifests/chunks, search
    /// records -- to every configured peer, so a peer that was offline
    /// during the original publish, or joined the mesh afterward, still
    /// converges instead of staying permanently missing that content.
    /// This is unconditional re-push, not a smarter "only send what's
    /// missing" reconciliation -- simple and correct, at the cost of
    /// re-sending unchanged content on every tick.
    #[serde(default = "default_resync_interval_secs")]
    pub resync_interval_secs: u64,
}

fn default_resync_interval_secs() -> u64 {
    300
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchConfig {
    pub enabled: bool,
    pub auto_index: bool,
    pub share_search_records: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccountConfig {
    pub enabled: bool,
    pub quota_mb: u64,
    /// Usernames this node exempts from the storage quota entirely, both
    /// when accepting a pointer/chunks on their behalf and (client-side)
    /// before pushing. Each node operator opts into trusting these
    /// usernames locally -- it is not a network-wide grant, since anyone
    /// could otherwise race to claim a "reserved" name for free unlimited
    /// hosting from every peer. Remove "god" here if you don't want to
    /// grant it unlimited storage on this node.
    #[serde(default = "default_unlimited_usernames")]
    pub unlimited_usernames: Vec<String>,
}

fn default_unlimited_usernames() -> Vec<String> {
    vec!["god".to_string()]
}

impl AccountConfig {
    pub fn is_unlimited(&self, username: &str) -> bool {
        self.unlimited_usernames.iter().any(|name| name == username)
    }
}

impl Default for AccountConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            quota_mb: 100,
            unlimited_usernames: default_unlimited_usernames(),
        }
    }
}

/// A node's altruistic mail relay: any node will store a small amount of
/// signed, sealed mail on behalf of any username, without needing to
/// verify it owns that account, the same way it hosts opaque encrypted
/// vfs chunks for others. `quota_mb` bounds how much of that a node will
/// carry per recipient; it is intentionally separate from
/// `[account].quota_mb` (vfs storage) so a full mailbox and a full vfs
/// don't compete with or shadow each other.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MailConfig {
    pub enabled: bool,
    pub quota_mb: u64,
}

impl Default for MailConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            quota_mb: 20,
        }
    }
}

/// A node's altruistic channel relay: any node will store a small
/// amount of signed (not sealed -- channels have no single recipient
/// key to seal against) chat traffic for any channel name, the same
/// no-questions-asked hosting posture `MailConfig` already documents.
/// Separate quota from both `[account]` and `[mail]` so none of the
/// three compete with or shadow each other.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatConfig {
    pub enabled: bool,
    pub quota_mb: u64,
}

impl Default for ChatConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            quota_mb: 20,
        }
    }
}

/// A node's altruistic board relay: any node will store a small amount
/// of signed (not sealed -- same reasoning as `ChatConfig`) posts and
/// comments for any board name, no-questions-asked, the same posture
/// `MailConfig`/`ChatConfig` already document. Separate quota from the
/// others so none compete with or shadow each other. Covers combined
/// post + comment bytes for a board.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BoardConfig {
    pub enabled: bool,
    pub quota_mb: u64,
}

impl Default for BoardConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            quota_mb: 50,
        }
    }
}

/// A node's altruistic file-transfer relay: sealed file bytes (never
/// plaintext -- see `rymnet-mail`'s `AttachmentRef`) that any node will
/// store on behalf of a specific intended recipient, the same
/// no-questions-asked hosting posture `MailConfig`/`ChatConfig` already
/// document. Separate quota from `[account]`/`[mail]`/`[chat]` so none
/// of the four compete with or shadow each other. Unlike the others,
/// this one also caps a *single* transfer's size (`max_file_size_mb`) --
/// worth having explicitly here since `SiteConfig.max_file_size_mb`
/// turned out to be defined but never actually enforced anywhere in this
/// codebase; this feature's version is real, checked where chunks are
/// offered.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileConfig {
    pub enabled: bool,
    pub quota_mb: u64,
    pub max_file_size_mb: u64,
}

impl Default for FileConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            quota_mb: 500,
            max_file_size_mb: 100,
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            node: NodeConfig {
                data_dir: "~/.rymnet".to_string(),
                listen_addr: "0.0.0.0:7444".to_string(),
                share_peer_list: true,
            },
            gateway: GatewayConfig {
                enabled: true,
                listen_addr: "127.0.0.1:7447".to_string(),
            },
            admin: AdminConfig {
                enabled: true,
                listen_addr: "127.0.0.1:7448".to_string(),
            },
            site: SiteConfig {
                max_site_size_mb: 50,
                max_file_size_mb: 10,
                allow_images: true,
                allow_fonts: false,
            },
            replication: ReplicationConfig {
                enabled: true,
                default_copies: 3,
                max_storage_mb: 1024,
                resync_interval_secs: default_resync_interval_secs(),
            },
            search: SearchConfig {
                enabled: true,
                auto_index: true,
                share_search_records: true,
            },
            account: AccountConfig::default(),
            mail: MailConfig::default(),
            chat: ChatConfig::default(),
            board: BoardConfig::default(),
            files: FileConfig::default(),
        }
    }
}

impl AppConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let text = fs::read_to_string(path.as_ref())
            .with_context(|| format!("read config {}", path.as_ref().display()))?;
        toml::from_str(&text).context("parse config")
    }

    pub fn save_new(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if path.exists() {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        let text = toml::to_string_pretty(self).context("serialize config")?;
        fs::write(path, text).with_context(|| format!("write config {}", path.display()))
    }
}
