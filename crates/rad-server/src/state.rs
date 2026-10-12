use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use rad_api::{Event, Id, ProviderHealth};
use rad_vault::{now_ms, Connection, Vault};
use tokio::sync::{broadcast, Notify};

use crate::devices::{random_hex, DeviceStore};
use crate::error::{ApiErr, ApiResult};

#[derive(Debug, Clone)]
pub struct Config {
    pub name: String,
    pub vault_path: PathBuf,
    pub backup_dir: PathBuf,
    pub ffmpeg: String,
    pub max_upload_bytes: u64,
    /// Loopback address of the internal media endpoint used to feed ffmpeg without temp files.
    pub media_addr: Option<std::net::SocketAddr>,
}

pub struct Session {
    pub device_id: String,
    pub last_used: i64,
}

const SESSION_IDLE_MS: i64 = 24 * 3600 * 1000;

pub struct UnlockLimiter {
    pub failures: u32,
    pub not_before: i64,
}

pub struct AppState {
    pub cfg: Config,
    pub devices: DeviceStore,
    vault: RwLock<Option<Arc<Vault>>>,
    sessions: Mutex<HashMap<String, Session>>,
    pairing: Mutex<Option<(String, i64)>>,
    pub events: broadcast::Sender<Event>,
    pub llm: rad_llm::Llm,
    pub jobs_wake: Notify,
    pub limiter: Mutex<UnlockLimiter>,
    pub health: RwLock<Vec<ProviderHealth>>,
    pub last_activity: AtomicI64,
    /// Short-lived tokens → blob ids for the loopback media endpoint.
    pub media_tokens: Mutex<HashMap<String, (Arc<Vault>, i64, String)>>,
    pub ffmpeg_ok: std::sync::atomic::AtomicBool,
    stopped: std::sync::atomic::AtomicBool,
    pub stop_notify: Notify,
}

pub type Shared = Arc<AppState>;

impl AppState {
    pub fn new(cfg: Config, devices: DeviceStore) -> Shared {
        let (events, _) = broadcast::channel(512);
        Arc::new(AppState {
            cfg,
            devices,
            vault: RwLock::new(None),
            sessions: Mutex::new(HashMap::new()),
            pairing: Mutex::new(None),
            events,
            llm: rad_llm::Llm::new(),
            jobs_wake: Notify::new(),
            limiter: Mutex::new(UnlockLimiter { failures: 0, not_before: 0 }),
            health: RwLock::new(Vec::new()),
            last_activity: AtomicI64::new(now_ms()),
            media_tokens: Mutex::new(HashMap::new()),
            ffmpeg_ok: std::sync::atomic::AtomicBool::new(false),
            stopped: std::sync::atomic::AtomicBool::new(false),
            stop_notify: Notify::new(),
        })
    }

    pub fn current_vault(&self) -> Option<Arc<Vault>> {
        self.vault.read().unwrap().clone()
    }

    pub fn vault(&self) -> ApiResult<Arc<Vault>> {
        self.current_vault().ok_or_else(ApiErr::locked)
    }

    pub fn is_initialized(&self) -> bool {
        Vault::exists(&self.cfg.vault_path)
    }

    pub fn set_unlocked(&self, v: Vault) {
        let v = Arc::new(v);
        if let Ok(c) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| v.conn())) {
            if let Err(e) = rad_vault::repo::requeue_running_jobs(&c) {
                tracing::warn!("requeue jobs: {e}");
            }
        }
        *self.vault.write().unwrap() = Some(v);
        self.touch();
        let _ = self.events.send(Event::Unlocked);
        self.jobs_wake.notify_waiters();
        tracing::info!("vault unlocked");
    }

    /// Removes the key from memory. Running jobs finish with their Arc and then drop it.
    pub fn lock(&self) {
        let had = self.vault.write().unwrap().take().is_some();
        self.sessions.lock().unwrap().clear();
        self.media_tokens.lock().unwrap().clear();
        if had {
            let _ = self.events.send(Event::Locked);
            tracing::info!("vault locked");
        }
    }

    /// Ends background work for good (standalone mode switching vaults / quitting).
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.stop_notify.notify_waiters();
        self.jobs_wake.notify_waiters();
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    pub fn touch(&self) {
        self.last_activity.store(now_ms(), Ordering::Relaxed);
    }

    // -------------------------------------------------------------- sessions

    pub fn new_session(&self, device_id: &str) -> String {
        let token = format!("rads_{}", random_hex(32));
        let mut s = self.sessions.lock().unwrap();
        let now = now_ms();
        s.retain(|_, v| now - v.last_used < SESSION_IDLE_MS);
        s.insert(token.clone(), Session { device_id: device_id.to_string(), last_used: now });
        token
    }

    pub fn check_session(&self, token: &str, device_id: &str) -> bool {
        let mut s = self.sessions.lock().unwrap();
        let now = now_ms();
        match s.get_mut(token) {
            Some(sess) if sess.device_id == device_id && now - sess.last_used < SESSION_IDLE_MS => {
                sess.last_used = now;
                true
            }
            Some(_) => {
                s.remove(token);
                false
            }
            None => false,
        }
    }

    pub fn end_session(&self, token: &str) {
        self.sessions.lock().unwrap().remove(token);
    }

    pub fn end_device_sessions(&self, device_id: &str) {
        self.sessions.lock().unwrap().retain(|_, v| v.device_id != device_id);
    }

    // -------------------------------------------------------------- pairing

    pub fn new_pairing_code(&self, ttl_ms: i64) -> (String, i64) {
        use rand::Rng;
        const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
        let mut rng = rand::rngs::OsRng;
        let raw: String = (0..8).map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char).collect();
        let code = format!("{}-{}", &raw[..4], &raw[4..]);
        let exp = now_ms() + ttl_ms;
        *self.pairing.lock().unwrap() = Some((code.clone(), exp));
        (code, exp)
    }

    /// Single-use, constant-time comparison.
    pub fn consume_pairing_code(&self, code: &str) -> bool {
        let norm = |s: &str| s.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_uppercase();
        let mut p = self.pairing.lock().unwrap();
        match p.as_ref() {
            Some((c, exp)) if *exp > now_ms() => {
                let a = blake3::hash(norm(c).as_bytes());
                let b = blake3::hash(norm(code).as_bytes());
                if a == b {
                    *p = None;
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    // -------------------------------------------------------------- unlock rate limit

    pub fn unlock_allowed(&self) -> ApiResult<()> {
        let l = self.limiter.lock().unwrap();
        let now = now_ms();
        if l.not_before > now {
            let secs = (l.not_before - now + 999) / 1000;
            return Err(ApiErr::new(
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                format!("too many attempts — try again in {secs}s"),
            ));
        }
        Ok(())
    }

    pub fn unlock_result(&self, ok: bool) {
        let mut l = self.limiter.lock().unwrap();
        if ok {
            l.failures = 0;
            l.not_before = 0;
        } else {
            l.failures += 1;
            if l.failures >= 3 {
                let delay = 1000i64 << (l.failures - 3).min(9);
                l.not_before = now_ms() + delay.min(300_000);
            }
        }
    }

    // -------------------------------------------------------------- events

    /// Records a durable change and broadcasts it. Call inside the same blocking section as the write.
    pub fn changed(&self, c: &Connection, entity: &str, id: Id, op: &str) {
        match rad_vault::repo::log_change(c, entity, id, op) {
            Ok(seq) => {
                let _ = self.events.send(Event::Change { seq, entity: entity.into(), entity_id: id, op: op.into() });
            }
            Err(e) => tracing::warn!("change log: {e}"),
        }
    }
}

pub async fn db_vault<T, F>(v: Arc<Vault>, state: &Shared, f: F) -> ApiResult<T>
where
    T: Send + 'static,
    F: FnOnce(&AppState, &Connection) -> ApiResult<T> + Send + 'static,
{
    let st = state.clone();
    tokio::task::spawn_blocking(move || {
        let c = v.conn();
        f(&st, &c)
    })
    .await
    .map_err(ApiErr::internal)?
}
