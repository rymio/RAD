use anyhow::{Context, Result};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct DataLayout {
    pub root: PathBuf,
}

impl DataLayout {
    pub fn default() -> Result<Self> {
        Ok(Self {
            root: expand_tilde("~/.rymnet")?,
        })
    }

    pub fn from_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn config_path(&self) -> PathBuf {
        self.root.join("config.toml")
    }

    pub fn identity_path(&self) -> PathBuf {
        self.root.join("identity").join("node_key.json")
    }

    pub fn peers_path(&self) -> PathBuf {
        self.root.join("peers").join("peers.toml")
    }

    pub fn domain_claims_dir(&self) -> PathBuf {
        self.root.join("domains").join("claims")
    }

    pub fn site_root(&self) -> PathBuf {
        self.root.join("sites")
    }

    pub fn chunks_root(&self) -> PathBuf {
        self.root.join("chunks").join("blake3")
    }

    pub fn search_root(&self) -> PathBuf {
        self.root.join("search")
    }

    pub fn cache_root(&self) -> PathBuf {
        self.root.join("cache")
    }

    pub fn logs_root(&self) -> PathBuf {
        self.root.join("logs")
    }

    pub fn accounts_root(&self) -> PathBuf {
        self.root.join("accounts")
    }

    pub fn account_claims_dir(&self) -> PathBuf {
        self.accounts_root().join("claims")
    }

    pub fn account_pointers_dir(&self) -> PathBuf {
        self.accounts_root().join("pointers")
    }

    /// Per-username ledger a hosting peer uses to enforce the storage
    /// quota for accounts it is not the owner of.
    pub fn account_hosted_dir(&self) -> PathBuf {
        self.accounts_root().join("hosted")
    }

    /// Local working copy of a logged-in user's decrypted virtual computer.
    /// This is scratch space rebuilt from the network on login; it is not
    /// itself the source of truth.
    pub fn account_cache_dir(&self, username: &str) -> PathBuf {
        self.accounts_root().join("cache").join(username)
    }

    pub fn mail_root(&self) -> PathBuf {
        self.root.join("mail")
    }

    /// Published mailbox encryption public keys, keyed by username --
    /// anyone can fetch these to seal a message for that user.
    pub fn mailbox_keys_dir(&self) -> PathBuf {
        self.mail_root().join("keys")
    }

    /// Sealed mail this node is relaying on behalf of `username`,
    /// regardless of whether this node has any other relationship with
    /// that account.
    pub fn mail_inbox_dir(&self, username: &str) -> PathBuf {
        self.mail_root().join("inbox").join(username)
    }

    /// Per-username ledger a hosting peer uses to enforce `[mail].quota_mb`
    /// for mail it relays on behalf of accounts it does not own.
    pub fn mail_hosted_dir(&self) -> PathBuf {
        self.mail_root().join("hosted")
    }

    pub fn chat_root(&self) -> PathBuf {
        self.root.join("chat")
    }

    /// Signed channel messages this node is relaying, regardless of
    /// whether this node has any other relationship with that channel --
    /// channels have no ownership/claim, so any node may host any of
    /// them, the same posture `mail_inbox_dir` already takes for mail.
    /// `channel_dir_name` must be the already-sanitized name (leading
    /// `#` stripped, validated) from `rymnet_chat::channel_dir_name`,
    /// never a raw, unchecked string.
    pub fn chat_channel_dir(&self, channel_dir_name: &str) -> PathBuf {
        self.chat_root().join("channels").join(channel_dir_name)
    }

    pub fn files_root(&self) -> PathBuf {
        self.root.join("files")
    }

    /// Per-recipient ledger a hosting peer uses to enforce
    /// `[files].quota_mb` for sealed file chunks it relays on someone's
    /// behalf. Unlike mail, there's no `files_inbox_dir` alongside this --
    /// the sealed bytes themselves live in the shared, global
    /// `ChunkStore` at `chunks_root()` (the same pool `.rym` site assets
    /// use), addressed purely by content hash; only the quota bookkeeping
    /// needs a per-recipient home.
    pub fn files_hosted_dir(&self) -> PathBuf {
        self.files_root().join("hosted")
    }

    /// Per-channel ledger a hosting peer uses to enforce `[chat].quota_mb`.
    pub fn chat_hosted_dir(&self) -> PathBuf {
        self.chat_root().join("hosted")
    }

    pub fn board_root(&self) -> PathBuf {
        self.root.join("boards")
    }

    /// A board's own directory -- same "no ownership/claim, any node may
    /// host it" posture as `chat_channel_dir`. `board_dir_name` must be
    /// the already-validated name from `rymnet_board::board_dir_name`,
    /// never a raw, unchecked string.
    pub fn board_dir(&self, board_dir_name: &str) -> PathBuf {
        self.board_root().join("boards").join(board_dir_name)
    }

    /// The board's append-only post index -- one compact `PostSummary`
    /// JSON line per post, never rewritten in place. What list/poll
    /// requests actually read; full post bodies are read individually,
    /// lazily, only when a specific post is opened. See
    /// `rymnet_board::post::PostSummary`'s doc comment for why this
    /// exists as a separate file rather than scanning `board_posts_dir`.
    pub fn board_index_path(&self, board_dir_name: &str) -> PathBuf {
        self.board_dir(board_dir_name).join("index.jsonl")
    }

    pub fn board_posts_dir(&self, board_dir_name: &str) -> PathBuf {
        self.board_dir(board_dir_name).join("posts")
    }

    pub fn board_post_path(&self, board_dir_name: &str, post_id: &str) -> PathBuf {
        self.board_posts_dir(board_dir_name).join(format!("{post_id}.json"))
    }

    pub fn board_comments_dir(&self, board_dir_name: &str, post_id: &str) -> PathBuf {
        self.board_posts_dir(board_dir_name).join(post_id).join("comments")
    }

    /// Per-board ledger a hosting peer uses to enforce `[board].quota_mb`,
    /// covering combined post + comment bytes.
    pub fn board_hosted_dir(&self) -> PathBuf {
        self.board_root().join("hosted")
    }

    pub fn ensure_all(&self) -> Result<()> {
        let dirs = [
            self.root.clone(),
            self.identity_path()
                .parent()
                .expect("identity parent")
                .to_path_buf(),
            self.peers_path()
                .parent()
                .expect("peers parent")
                .to_path_buf(),
            self.domain_claims_dir(),
            self.site_root(),
            self.chunks_root(),
            self.search_root(),
            self.cache_root(),
            self.logs_root(),
            self.account_claims_dir(),
            self.account_pointers_dir(),
            self.account_hosted_dir(),
            self.mailbox_keys_dir(),
            self.mail_hosted_dir(),
            self.chat_hosted_dir(),
            self.board_hosted_dir(),
            self.files_hosted_dir(),
        ];

        for dir in &dirs {
            fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        }

        if !self.peers_path().exists() {
            fs::write(self.peers_path(), "# Rymnet peers\n\n")
                .with_context(|| format!("write {}", self.peers_path().display()))?;
        }

        Ok(())
    }
}

pub fn expand_tilde(path: &str) -> Result<PathBuf> {
    if path == "~" {
        return home_dir();
    }

    if let Some(rest) = path.strip_prefix("~/") {
        return Ok(home_dir()?.join(rest));
    }

    Ok(Path::new(path).to_path_buf())
}

fn home_dir() -> Result<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("HOME is not set"))
}
