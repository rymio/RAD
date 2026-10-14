use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};

use rusqlite::Connection;
use zeroize::Zeroizing;

use crate::{Result, VaultError};

pub(crate) fn open_conn(path: &Path, key_hex: &str) -> Result<Connection> {
    let c = Connection::open(path)?;
    c.execute_batch(&format!("PRAGMA key = \"x'{key_hex}'\";"))?;
    // Fails with SQLITE_NOTADB if the key is wrong.
    match c.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get::<_, i64>(0)) {
        Ok(_) => {}
        Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == rusqlite::ErrorCode::NotADatabase => {
            return Err(VaultError::WrongKey)
        }
        Err(e) => return Err(e.into()),
    }
    // A long busy timeout: on a shared vault, a write waits its turn behind other computers'.
    c.execute_batch(
        "PRAGMA foreign_keys=ON;
         PRAGMA synchronous=NORMAL;
         PRAGMA temp_store=MEMORY;
         PRAGMA busy_timeout=60000;
         PRAGMA cache_size=-32000;",
    )?;
    Ok(c)
}

struct Inner {
    idle: Vec<Connection>,
    total: usize,
    key_hex: Zeroizing<String>,
    exclusive: bool,
}

pub(crate) struct Pool {
    path: PathBuf,
    max: usize,
    inner: Mutex<Inner>,
    cv: Condvar,
}

impl Pool {
    pub fn new(path: PathBuf, key_hex: Zeroizing<String>, first: Connection, max: usize) -> Pool {
        Pool {
            path,
            max,
            inner: Mutex::new(Inner { idle: vec![first], total: 1, key_hex, exclusive: false }),
            cv: Condvar::new(),
        }
    }

    pub fn get(&self) -> PooledConn<'_> {
        let mut g = self.inner.lock().unwrap();
        loop {
            if !g.exclusive {
                if let Some(c) = g.idle.pop() {
                    return PooledConn { pool: self, conn: Some(c) };
                }
                if g.total < self.max {
                    g.total += 1;
                    let key = g.key_hex.clone();
                    drop(g);
                    match open_conn(&self.path, &key) {
                        Ok(c) => return PooledConn { pool: self, conn: Some(c) },
                        Err(e) => {
                            tracing::error!("opening vault connection failed: {e}");
                            g = self.inner.lock().unwrap();
                            g.total -= 1;
                        }
                    }
                }
            }
            g = self.cv.wait(g).unwrap();
        }
    }

    /// Waits until every connection is returned, closes them, runs `f` on a single fresh
    /// connection, then switches the pool to `new_key_hex`.
    pub fn exclusive<F>(&self, f: F, new_key_hex: Zeroizing<String>) -> Result<()>
    where
        F: FnOnce(&Connection) -> Result<()>,
    {
        let mut g = self.inner.lock().unwrap();
        while g.exclusive {
            g = self.cv.wait(g).unwrap();
        }
        g.exclusive = true;
        while g.idle.len() < g.total {
            g = self.cv.wait(g).unwrap();
        }
        g.idle.clear();
        g.total = 0;
        let old_key = g.key_hex.clone();
        drop(g);

        let result = (|| {
            let c = open_conn(&self.path, &old_key)?;
            f(&c)?;
            drop(c);
            open_conn(&self.path, &new_key_hex)
        })();

        let mut g = self.inner.lock().unwrap();
        g.exclusive = false;
        let out = match result {
            Ok(c) => {
                g.key_hex = new_key_hex;
                g.idle.push(c);
                g.total = 1;
                Ok(())
            }
            Err(e) => {
                if let Ok(c) = open_conn(&self.path, &old_key) {
                    g.idle.push(c);
                    g.total = 1;
                }
                Err(e)
            }
        };
        self.cv.notify_all();
        out
    }

    fn put(&self, c: Connection) {
        let mut g = self.inner.lock().unwrap();
        if g.exclusive && g.total > 0 {
            // Being drained for exclusive access; the drainer counts idle connections.
        }
        g.idle.push(c);
        self.cv.notify_all();
    }
}

pub struct PooledConn<'a> {
    pool: &'a Pool,
    conn: Option<Connection>,
}

impl Deref for PooledConn<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        self.conn.as_ref().unwrap()
    }
}

impl DerefMut for PooledConn<'_> {
    fn deref_mut(&mut self) -> &mut Connection {
        self.conn.as_mut().unwrap()
    }
}

impl Drop for PooledConn<'_> {
    fn drop(&mut self) {
        if let Some(c) = self.conn.take() {
            self.pool.put(c);
        }
    }
}
