//! The RAD vault: a single SQLCipher-encrypted file holding every project, document blob,
//! note, search index, chat and setting.

mod crypto;
mod pool;
mod schema;

pub mod blobs;
pub mod repo;
pub mod search;

use std::path::{Path, PathBuf};
use std::sync::{Mutex, Once};

use zeroize::Zeroizing;

pub use crypto::{decode_recovery_key, encode_recovery_key};
pub use pool::PooledConn;
pub use rusqlite;
pub use rusqlite::Connection;

use pool::Pool;

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("wrong passphrase or recovery key")]
    WrongKey,
    #[error("vault file not found: {0}")]
    NotFound(String),
    #[error("vault already exists: {0}")]
    AlreadyExists(String),
    #[error("not a RAD vault (or the file is damaged)")]
    NotAVault,
    #[error("invalid recovery key")]
    BadRecoveryKey,
    #[error("passphrase too short (minimum {0} characters)")]
    WeakPassphrase(usize),
    #[error("not found")]
    Missing,
    #[error("{0}")]
    Invalid(String),
    #[error("database error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, VaultError>;

pub const MIN_PASSPHRASE: usize = 10;
pub const FORMAT_VERSION: i64 = 1;

static VEC_INIT: Once = Once::new();

fn register_extensions() {
    VEC_INIT.call_once(|| unsafe {
        #[allow(clippy::missing_transmute_annotations)]
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite_vec::sqlite3_vec_init as *const (),
        )));
    });
}

/// Runs `f` in a write transaction (`BEGIN IMMEDIATE`, so concurrent writers wait on the busy
/// timeout instead of failing), or inside the caller's transaction if one is already open.
pub fn in_tx<T>(c: &Connection, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
    if !c.is_autocommit() {
        return f(c);
    }
    let tx = rusqlite::Transaction::new_unchecked(c, rusqlite::TransactionBehavior::Immediate)?;
    let out = f(&tx)?;
    tx.commit()?;
    Ok(out)
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub struct Vault {
    path: PathBuf,
    key: Mutex<Zeroizing<[u8; 32]>>,
    pool: Pool,
}

impl std::fmt::Debug for Vault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vault").field("path", &self.path).finish_non_exhaustive()
    }
}

impl Vault {
    pub fn exists(path: &Path) -> bool {
        path.is_file()
    }

    /// Creates a new vault and returns it together with its recovery key.
    pub fn create(path: &Path, passphrase: &str) -> Result<(Vault, String)> {
        register_extensions();
        if passphrase.chars().count() < MIN_PASSPHRASE {
            return Err(VaultError::WeakPassphrase(MIN_PASSPHRASE));
        }
        if path.exists() {
            return Err(VaultError::AlreadyExists(path.display().to_string()));
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let salt = crypto::random_salt();
        let key = crypto::derive_key(passphrase, &salt);
        {
            let c = Connection::open(path)?;
            c.execute_batch(&format!(
                "PRAGMA key = \"x'{}{}'\";",
                hex::encode(&*key),
                hex::encode(salt)
            ))?;
            c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA auto_vacuum=INCREMENTAL;")?;
            schema::migrate(&c)?;
            c.execute(
                "INSERT OR REPLACE INTO vault_meta(key, value) VALUES ('created_at', ?1), ('format_version', ?2)",
                rusqlite::params![now_ms().to_string(), FORMAT_VERSION.to_string()],
            )?;
        }
        let vault = Self::open_with_key(path, key.clone())?;
        let recovery = crypto::encode_recovery_key(&key);
        Ok((vault, recovery))
    }

    pub fn open(path: &Path, passphrase: &str) -> Result<Vault> {
        let salt = read_salt(path)?;
        let key = crypto::derive_key(passphrase, &salt);
        Self::open_with_key(path, key)
    }

    pub fn open_with_recovery(path: &Path, recovery_key: &str) -> Result<Vault> {
        let key = crypto::decode_recovery_key(recovery_key)?;
        Self::open_with_key(path, key)
    }

    fn open_with_key(path: &Path, key: Zeroizing<[u8; 32]>) -> Result<Vault> {
        register_extensions();
        if !path.is_file() {
            return Err(VaultError::NotFound(path.display().to_string()));
        }
        let key_hex = Zeroizing::new(hex::encode(&*key));
        let first = pool::open_conn(path, &key_hex)?;
        // Ensure the schema is current before handing out connections.
        schema::migrate(&first)?;
        let pool = Pool::new(path.to_path_buf(), key_hex, first, 6);
        let v = Vault { path: path.to_path_buf(), key: Mutex::new(key), pool };
        {
            let c = v.conn();
            c.execute_batch("PRAGMA journal_mode=WAL;")?;
        }
        Ok(v)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A pooled connection. Blocks until one is free.
    pub fn conn(&self) -> PooledConn<'_> {
        self.pool.get()
    }

    /// Constant-time check that `passphrase` is the vault's current passphrase.
    pub fn verify_passphrase(&self, passphrase: &str) -> bool {
        let Ok(salt) = read_salt(&self.path) else { return false };
        let candidate = crypto::derive_key(passphrase, &salt);
        let key = self.key.lock().unwrap();
        subtle::ConstantTimeEq::ct_eq(&candidate[..], &key[..]).into()
    }

    /// Re-encrypts the vault under a new passphrase. Returns the new recovery key
    /// (the previous recovery key stops working).
    pub fn rekey(&self, new_passphrase: &str) -> Result<String> {
        if new_passphrase.chars().count() < MIN_PASSPHRASE {
            return Err(VaultError::WeakPassphrase(MIN_PASSPHRASE));
        }
        let salt = read_salt(&self.path)?;
        let new_key = crypto::derive_key(new_passphrase, &salt);
        let new_hex = Zeroizing::new(hex::encode(&*new_key));
        let rekey_sql = Zeroizing::new(format!("PRAGMA rekey = \"x'{}'\";", &*new_hex));
        self.pool.exclusive(
            move |c| {
                c.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
                c.execute_batch(&rekey_sql)?;
                Ok(())
            },
            new_hex,
        )?;
        *self.key.lock().unwrap() = new_key.clone();
        Ok(crypto::encode_recovery_key(&new_key))
    }

    /// Online backup into a new single-file vault encrypted with the same key.
    pub fn backup_to(&self, dest: &Path) -> Result<()> {
        if dest.exists() {
            return Err(VaultError::AlreadyExists(dest.display().to_string()));
        }
        if let Some(dir) = dest.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let salt = read_salt(&self.path)?;
        let key_hex = {
            let k = self.key.lock().unwrap();
            Zeroizing::new(format!("{}{}", hex::encode(&**k), hex::encode(salt)))
        };
        let src = self.conn();
        let mut dst = Connection::open(dest)?;
        dst.execute_batch(&format!("PRAGMA key = \"x'{}'\";", &*key_hex))?;
        {
            let b = rusqlite::backup::Backup::new(&src, &mut dst)?;
            b.run_to_completion(256, std::time::Duration::from_millis(5), None)?;
        }
        dst.execute_batch("PRAGMA journal_mode=DELETE;")?;
        Ok(())
    }

    pub fn integrity_check(&self) -> Result<Vec<String>> {
        let c = self.conn();
        let mut out = Vec::new();
        {
            let mut st = c.prepare("PRAGMA cipher_integrity_check")?;
            let rows = st.query_map([], |r| r.get::<_, String>(0))?;
            for r in rows {
                out.push(format!("cipher: {}", r?));
            }
        }
        let mut st = c.prepare("PRAGMA integrity_check")?;
        let rows = st.query_map([], |r| r.get::<_, String>(0))?;
        for r in rows {
            let s = r?;
            if s != "ok" || out.is_empty() {
                out.push(s);
            }
        }
        Ok(out)
    }

    pub fn compact(&self) -> Result<()> {
        let c = self.conn();
        c.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA incremental_vacuum;")?;
        Ok(())
    }

    pub fn file_size(&self) -> u64 {
        let mut total = std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
        let wal = PathBuf::from(format!("{}-wal", self.path.display()));
        total += std::fs::metadata(wal).map(|m| m.len()).unwrap_or(0);
        total
    }

    /// Checkpoints the WAL so the vault is a single self-contained file again.
    pub fn close(self) {
        if let Ok(()) = self.conn().execute_batch("PRAGMA wal_checkpoint(TRUNCATE);") {}
        drop(self);
    }
}

/// SQLCipher stores its 16-byte KDF salt in the first bytes of the file; we reuse it as
/// the Argon2id salt so the vault stays a single file.
fn read_salt(path: &Path) -> Result<[u8; 16]> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => VaultError::NotFound(path.display().to_string()),
        _ => VaultError::Io(e),
    })?;
    let mut salt = [0u8; 16];
    f.read_exact(&mut salt).map_err(|_| VaultError::NotAVault)?;
    if &salt == b"SQLite format 3\0" {
        // An unencrypted SQLite file is never a RAD vault.
        return Err(VaultError::NotAVault);
    }
    Ok(salt)
}

#[cfg(test)]
mod tests;
