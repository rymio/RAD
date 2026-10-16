use crate::vfs::VirtualComputer;
use rymnet_account::CIPHER_KEY_LEN;
use rymnet_core::NodeIdentity;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use x25519_dalek::StaticSecret;

/// A logged-in-ness marker plus the keys needed to act as that account.
pub struct AccountState {
    pub username: String,
    pub identity: NodeIdentity,
    pub cipher_key: [u8; CIPHER_KEY_LEN],
    pub mailbox_secret: StaticSecret,
    pub vfs_version: u64,
    pub cache_dir: PathBuf,
}

/// A multi-step interaction in progress, mirroring `rymsh`'s sequential
/// prompts (`To:`, `Cc:`, `Subject:`, blank-line-terminated body, ...).
/// The next line typed into the terminal is consumed as the answer to
/// whichever step this holds, rather than parsed as a new command.
pub enum Pending {
    ComposeCc {
        to: String,
    },
    ComposeBcc {
        to: String,
        cc: Vec<String>,
    },
    ComposeSubject {
        to: String,
        cc: Vec<String>,
        bcc: Vec<String>,
    },
    ComposeBody {
        to: String,
        cc: Vec<String>,
        bcc: Vec<String>,
        subject: String,
        lines: Vec<String>,
        in_reply_to: Option<String>,
        thread_id: Option<String>,
    },
    EditBody {
        path: String,
        lines: Vec<String>,
    },
    ForwardTo {
        original_index: usize,
    },
    ForwardNote {
        to: String,
        original_index: usize,
        lines: Vec<String>,
    },
}

pub struct WebSession {
    pub account: Option<AccountState>,
    pub vfs: VirtualComputer,
    pub pending: Option<Pending>,
    pub last_inbox: Vec<crate::mail::InboxMessage>,
    pub last_seen: Instant,
}

impl WebSession {
    pub fn identity<'a>(&'a self, server_identity: &'a NodeIdentity) -> &'a NodeIdentity {
        self.account
            .as_ref()
            .map(|a| &a.identity)
            .unwrap_or(server_identity)
    }
}

pub type SessionStore = Arc<RwLock<HashMap<String, WebSession>>>;
