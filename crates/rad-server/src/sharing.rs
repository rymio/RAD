//! One vault file used by several computers at once (on a network share).
//!
//! Every RAD that has the vault unlocked announces itself next to the file (see
//! `rad_vault::presence`). Before opening, [`check_can_open`] refuses when another computer
//! has the vault open alone, or when the file is in a sync folder, where two computers can't
//! safely share it. While unlocked, [`run`] keeps this computer's entry fresh and, for a
//! shared vault, turns other computers' changes into live events, so their notes and chats
//! appear here without a reload. Writes from different computers queue on the database lock.

use std::sync::Arc;
use std::time::Duration;

use rad_api::{Event, VaultSession, VaultSharing};
use rad_vault::location::{self, LocationKind};
use rad_vault::presence::{self, Session};
use rad_vault::{now_ms, repo, Vault};

use crate::error::{ApiErr, ApiResult};
use crate::state::{AppState, Shared};

/// This computer's id (as given, else kept in `state_dir/rad-machine-id`, else derived from
/// the host) and a name people recognise.
pub fn machine_identity(id: Option<String>, name: Option<String>, state_dir: Option<&std::path::Path>) -> (String, String) {
    let host = hostname::get().map(|h| h.to_string_lossy().into_owned()).unwrap_or_else(|_| "computer".into());
    let id = id.filter(|i| !i.is_empty()).or_else(|| {
        let f = state_dir?.join("rad-machine-id");
        if let Some(existing) = std::fs::read_to_string(&f).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
            return Some(existing);
        }
        let fresh = blake3::hash(format!("{host}\0{}\0{}", std::process::id(), now_ms()).as_bytes()).to_hex()[..20].to_string();
        std::fs::write(&f, &fresh).ok().map(|_| fresh)
    });
    let id = id.unwrap_or_else(|| {
        let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(|h| h.to_string_lossy().into_owned()).unwrap_or_default();
        blake3::hash(format!("{host}\0{home}").as_bytes()).to_hex()[..20].to_string()
    });
    let pretty = name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| host.trim_end_matches(".local").replace('-', " "));
    (id, pretty)
}

fn conflict(msg: String) -> ApiErr {
    ApiErr::new(axum::http::StatusCode::CONFLICT, "vault_in_use", msg)
}

fn names(list: &[Session]) -> String {
    let mut n: Vec<&str> = list.iter().map(|s| s.machine.as_str()).collect();
    n.dedup();
    n.join(", ")
}

/// Refuses to open the vault while that would be unsafe for another computer using it.
pub fn check_can_open(st: &AppState) -> ApiResult<()> {
    let path = &st.cfg.vault_path;
    let others = presence::others(path, &st.cfg.machine_id);
    if others.is_empty() {
        return Ok(());
    }
    let loc = location::classify(path);
    if loc.kind == LocationKind::Synced {
        return Err(conflict(format!(
            "This vault is open on {} right now. It's in {}, which copies the file between computers instead of sharing it — \
             opening it here too would create conflicting copies. Lock RAD there first (or wait a couple of minutes if it's off).",
            names(&others),
            loc.label
        )));
    }
    let alone: Vec<Session> = others.into_iter().filter(|s| !s.shared).collect();
    if !alone.is_empty() {
        return Err(conflict(format!(
            "This vault is open on {} right now. To use it on several computers at once, turn on \
             “Use on several computers” there (Settings → Storage), or lock RAD there first.",
            names(&alone)
        )));
    }
    Ok(())
}

/// After opening: a vault others are using must be in shared mode (consistency check).
pub fn check_opened(st: &AppState, v: &Vault) -> ApiResult<()> {
    if !presence::others(&st.cfg.vault_path, &st.cfg.machine_id).is_empty() && !v.is_shared() {
        return Err(conflict("Another computer has this vault open, but sharing is turned off in the vault. Lock RAD there first.".into()));
    }
    Ok(())
}

fn announce(st: &AppState, since: i64, shared: bool) {
    let s = Session {
        machine_id: st.cfg.machine_id.clone(),
        machine: st.cfg.machine_name.clone(),
        pid: std::process::id(),
        since,
        last_seen: now_ms(),
        shared,
    };
    if let Err(e) = presence::announce(&st.cfg.vault_path, &s) {
        tracing::warn!("couldn't update the vault's session file: {e}");
    }
}

pub fn withdraw(st: &AppState) {
    presence::withdraw(&st.cfg.vault_path, &st.cfg.machine_id);
}

/// Runs while `vault` is the unlocked vault: heartbeat, plus relaying other computers'
/// changes when shared.
pub async fn run(st: Shared, vault: Arc<Vault>) {
    let since = now_ms();
    let v = vault.clone();
    let (mut shared, mut cursor) = tokio::task::spawn_blocking(move || (v.is_shared(), repo::last_seq(&v.conn()).unwrap_or(0))).await.unwrap_or((false, 0));
    announce(&st, since, shared);
    let mut last_beat = now_ms();
    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            _ = st.stop_notify.notified() => break,
            _ = st.sharing_changed.notified() => {}
        }
        let current = st.current_vault();
        if st.is_stopped() || !current.as_ref().is_some_and(|c| Arc::ptr_eq(c, &vault)) {
            break;
        }
        if now_ms() - last_beat >= presence::HEARTBEAT_MS || st.sharing_dirty() {
            let v = vault.clone();
            shared = tokio::task::spawn_blocking(move || v.is_shared()).await.unwrap_or(shared);
            announce(&st, since, shared);
            last_beat = now_ms();
        }
        if !shared {
            continue;
        }
        // Other computers' writes: everything in the change log past our cursor that this
        // process didn't write itself.
        let v = vault.clone();
        let from = cursor;
        let Ok(Ok(entries)) = tokio::task::spawn_blocking(move || repo::changes_since(&v.conn(), from, 2000)).await else { continue };
        if entries.is_empty() {
            continue;
        }
        cursor = entries.last().map(|e| e.seq).unwrap_or(cursor);
        let mut remote = false;
        for e in entries {
            if st.take_own_seq(e.seq) {
                continue;
            }
            remote = true;
            let _ = st.events.send(Event::Change { seq: e.seq, entity: e.entity, entity_id: e.entity_id, op: e.op });
        }
        if remote {
            // They may have queued work (e.g. a document to index) this computer can help with.
            st.jobs_wake.notify_waiters();
        }
    }
    // Locked or switched vaults: we no longer have it open.
    if st.current_vault().is_none_or(|c| Arc::ptr_eq(&c, &vault)) {
        withdraw(&st);
    }
}

pub fn info(st: &AppState, shared: bool) -> VaultSharing {
    let loc = location::classify(&st.cfg.vault_path);
    VaultSharing {
        shared,
        location: match loc.kind {
            LocationKind::Local => "local",
            LocationKind::Network => "network",
            LocationKind::Synced => "synced",
        }
        .into(),
        location_label: loc.label,
        sessions: presence::active(&st.cfg.vault_path)
            .into_iter()
            .map(|s| VaultSession { this: s.machine_id == st.cfg.machine_id, machine: s.machine, since: s.since, last_seen: s.last_seen })
            .collect(),
    }
}

/// Turning sharing on needs a location other computers can safely reach; turning it off
/// needs everyone else to have closed the vault.
pub fn check_toggle(st: &AppState, on: bool) -> ApiResult<()> {
    if on {
        let loc = location::classify(&st.cfg.vault_path);
        if loc.kind == LocationKind::Synced {
            return Err(ApiErr::bad_request(format!(
                "The vault is in {}, which can't share one file between computers safely. Move it to a network share (a NAS or another computer's shared folder) first.",
                loc.label
            )));
        }
    } else {
        let others = presence::others(&st.cfg.vault_path, &st.cfg.machine_id);
        if !others.is_empty() {
            return Err(conflict(format!("{} still has the vault open. Lock RAD there first.", names(&others))));
        }
    }
    Ok(())
}

/// Re-keying (new passphrase or recovery) would lock out computers that have the vault open.
pub fn check_alone(st: &AppState, what: &str) -> ApiResult<()> {
    let others = presence::others(&st.cfg.vault_path, &st.cfg.machine_id);
    if others.is_empty() {
        Ok(())
    } else {
        Err(conflict(format!("{} has this vault open. Lock RAD there before you {what}.", names(&others))))
    }
}
