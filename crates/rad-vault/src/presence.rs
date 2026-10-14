//! Which computers have a vault open right now.
//!
//! Each computer that has the vault unlocked keeps a small file in a hidden folder next to
//! it (`.<name>.radvault.sessions/<machine>.json`) and rewrites it every
//! [`HEARTBEAT_MS`]. A file not refreshed for [`STALE_MS`] belongs to a computer that went
//! away without saying so (crash, sleep, unplugged network). The folder works without the
//! vault key and on every filesystem, so it can also warn about a second computer opening a
//! vault in a sync folder, where the database's own locks don't reach.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const HEARTBEAT_MS: i64 = 30_000;
/// Generous, so a sync service's delay doesn't make a live computer look gone.
pub const STALE_MS: i64 = 150_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub machine_id: String,
    /// The computer's name, for people.
    pub machine: String,
    pub pid: u32,
    pub since: i64,
    pub last_seen: i64,
    /// Opened in shared mode (others may join) rather than alone.
    #[serde(default)]
    pub shared: bool,
}

impl Session {
    pub fn is_fresh(&self, now: i64) -> bool {
        now - self.last_seen < STALE_MS
    }
}

pub fn dir(vault: &Path) -> PathBuf {
    let name = vault.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "vault".into());
    vault.with_file_name(format!(".{name}.sessions"))
}

fn file(vault: &Path, machine_id: &str) -> PathBuf {
    let safe: String = machine_id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(64).collect();
    dir(vault).join(format!("{safe}.json"))
}

/// Writes (or refreshes) this computer's entry.
pub fn announce(vault: &Path, s: &Session) -> std::io::Result<()> {
    let d = dir(vault);
    std::fs::create_dir_all(&d)?;
    let f = file(vault, &s.machine_id);
    let tmp = f.with_extension(format!("tmp{}", s.pid));
    std::fs::write(&tmp, serde_json::to_vec(s)?)?;
    std::fs::rename(&tmp, &f)
}

/// Removes this computer's entry (and the folder once it's empty).
pub fn withdraw(vault: &Path, machine_id: &str) {
    let _ = std::fs::remove_file(file(vault, machine_id));
    let _ = std::fs::remove_dir(dir(vault));
}

/// Every computer with a fresh entry, this one included.
pub fn active(vault: &Path) -> Vec<Session> {
    let now = crate::now_ms();
    let Ok(rd) = std::fs::read_dir(dir(vault)) else { return Vec::new() };
    let mut out: Vec<Session> = rd
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_slice::<Session>(&std::fs::read(e.path()).ok()?).ok())
        .filter(|s| s.is_fresh(now))
        .collect();
    out.sort_by(|a, b| a.machine.cmp(&b.machine));
    out
}

/// Other computers that have the vault open now.
pub fn others(vault: &Path, machine_id: &str) -> Vec<Session> {
    active(vault).into_iter().filter(|s| s.machine_id != machine_id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn announce_list_withdraw() {
        let td = tempfile::tempdir().unwrap();
        let v = td.path().join("Personal.radvault");
        let now = crate::now_ms();
        let me = Session { machine_id: "aaa".into(), machine: "MacBook".into(), pid: 1, since: now, last_seen: now, shared: true };
        let other = Session { machine_id: "bbb".into(), machine: "iMac".into(), pid: 2, since: now, last_seen: now, shared: true };
        let gone = Session { machine_id: "ccc".into(), machine: "Old".into(), pid: 3, since: 0, last_seen: now - STALE_MS - 1, shared: false };
        for s in [&me, &other, &gone] {
            announce(&v, s).unwrap();
        }
        assert!(dir(&v).ends_with(".Personal.radvault.sessions"));
        let names: Vec<String> = others(&v, "aaa").into_iter().map(|s| s.machine).collect();
        assert_eq!(names, vec!["iMac"]);
        withdraw(&v, "bbb");
        assert!(others(&v, "aaa").is_empty());
        assert_eq!(active(&v).len(), 1);
    }
}
