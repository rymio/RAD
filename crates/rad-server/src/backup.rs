//! Encrypted single-file backups, plus the server's periodic housekeeping tasks.

use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use rad_api::{BackupInfo, Event, ProviderHealth};
use rad_vault::{now_ms, repo, Vault};

use crate::error::{ApiErr, ApiResult};
use crate::models;
use crate::state::Shared;

pub fn list(dir: &Path) -> Vec<BackupInfo> {
    let mut out: Vec<BackupInfo> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".radvault"))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            let created = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as i64;
            Some(BackupInfo { file: e.file_name().to_string_lossy().into_owned(), bytes: m.len(), created_at: created })
        })
        .collect();
    out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    out
}

pub async fn run(state: &Shared, vault: Arc<Vault>, keep: u32) -> ApiResult<BackupInfo> {
    let dir = state.cfg.backup_dir.clone();
    let stem = state.cfg.vault_path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "vault".into());
    let name = format!("{stem}-{}.radvault", chrono::Local::now().format("%Y%m%d-%H%M%S"));
    let dest = dir.join(&name);
    let d2 = dest.clone();
    tokio::task::spawn_blocking(move || vault.backup_to(&d2)).await.map_err(ApiErr::internal)??;
    // Retention
    for old in list(&dir).into_iter().skip(keep as usize) {
        let _ = std::fs::remove_file(dir.join(old.file));
    }
    let bytes = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
    tracing::info!("backup written: {}", dest.display());
    Ok(BackupInfo { file: name, bytes, created_at: now_ms() })
}

/// Health checks, auto-lock, daily backups, change-log trimming.
pub fn spawn_housekeeping(state: Shared) {
    let st = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        loop {
            tokio::select! {
                _ = tick.tick() => {}
                _ = st.stop_notify.notified() => {}
            }
            if st.is_stopped() {
                return;
            }
            let Some(vault) = st.current_vault() else { continue };
            let v = vault.clone();
            let Ok(Ok((providers, settings))) =
                tokio::task::spawn_blocking(move || -> rad_vault::Result<_> { let c = v.conn(); Ok((repo::providers(&c)?, repo::settings(&c)?)) }).await
            else {
                continue;
            };

            // Provider health
            let mut health = Vec::new();
            for p in providers.iter().filter(|p| p.enabled) {
                let conn = models::conn(p);
                let r = st.llm.health(&conn).await;
                health.push(ProviderHealth {
                    provider_id: p.id,
                    ok: r.is_ok(),
                    latency_ms: r.as_ref().ok().map(|d| d.as_millis() as u64),
                    error: r.err().map(|e| e.to_string()),
                    checked_at: now_ms(),
                });
            }
            *st.health.write().unwrap() = health.clone();
            let _ = st.events.send(Event::ProviderHealth { health });

            // Auto-lock after inactivity
            if settings.server_auto_lock_minutes > 0 {
                let idle = now_ms() - st.last_activity.load(Ordering::Relaxed);
                if idle > settings.server_auto_lock_minutes as i64 * 60_000 {
                    tracing::info!("auto-locking vault after {} idle minutes", idle / 60_000);
                    st.lock();
                    continue;
                }
            }

            // Daily backup
            if settings.backup_daily {
                let last = list(&st.cfg.backup_dir).first().map(|b| b.created_at).unwrap_or(0);
                if now_ms() - last > 24 * 3600 * 1000 {
                    if let Err(e) = run(&st, vault.clone(), settings.backup_keep).await {
                        tracing::error!("scheduled backup failed: {}", e.message);
                    }
                }
            }

            let v = vault.clone();
            let _ = tokio::task::spawn_blocking(move || {
                let c = v.conn();
                let _ = repo::trim_change_log(&c, 50_000);
                let _ = rad_vault::blobs::collect_garbage(&c);
            })
            .await;
        }
    });
}
