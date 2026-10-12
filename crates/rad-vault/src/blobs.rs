//! Content-addressed file storage *inside* the vault, split into 1 MiB rows.

use rusqlite::{params, Connection, OptionalExtension};

use crate::{now_ms, Result, VaultError};

pub const CHUNK: usize = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct BlobMeta {
    pub id: i64,
    pub hash: String,
    pub size: i64,
    pub mime: String,
}

/// Streams a file into the vault. Call [`BlobWriter::write`] with any slice sizes and
/// [`BlobWriter::finish`] at the end. Deduplicates by BLAKE3 hash.
pub struct BlobWriter {
    blob_id: i64,
    hasher: blake3::Hasher,
    buf: Vec<u8>,
    seq: i64,
    size: u64,
}

impl BlobWriter {
    pub fn begin(c: &Connection) -> Result<BlobWriter> {
        c.execute("INSERT INTO blobs(created_at) VALUES (?1)", params![now_ms()])?;
        Ok(BlobWriter {
            blob_id: c.last_insert_rowid(),
            hasher: blake3::Hasher::new(),
            buf: Vec::with_capacity(CHUNK),
            seq: 0,
            size: 0,
        })
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn write(&mut self, c: &Connection, mut data: &[u8]) -> Result<()> {
        self.hasher.update(data);
        self.size += data.len() as u64;
        while !data.is_empty() {
            let take = (CHUNK - self.buf.len()).min(data.len());
            self.buf.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.buf.len() == CHUNK {
                self.flush(c)?;
            }
        }
        Ok(())
    }

    fn flush(&mut self, c: &Connection) -> Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        c.execute(
            "INSERT INTO blob_chunks(blob_id, seq, data) VALUES (?1, ?2, ?3)",
            params![self.blob_id, self.seq, &self.buf],
        )?;
        self.seq += 1;
        self.buf.clear();
        Ok(())
    }

    /// Returns the blob and whether an identical blob already existed (in which case the
    /// freshly written copy is discarded).
    pub fn finish(mut self, c: &Connection, mime: &str) -> Result<(BlobMeta, bool)> {
        self.flush(c)?;
        let hash = self.hasher.finalize().to_hex().to_string();
        if let Some(existing) = by_hash(c, &hash)? {
            c.execute("DELETE FROM blobs WHERE id = ?1", params![self.blob_id])?;
            return Ok((existing, true));
        }
        c.execute(
            "UPDATE blobs SET hash = ?1, size = ?2, mime = ?3, chunk_count = ?4, complete = 1 WHERE id = ?5",
            params![hash, self.size as i64, mime, self.seq, self.blob_id],
        )?;
        Ok((BlobMeta { id: self.blob_id, hash, size: self.size as i64, mime: mime.to_string() }, false))
    }

    pub fn abort(self, c: &Connection) {
        let _ = c.execute("DELETE FROM blobs WHERE id = ?1", params![self.blob_id]);
    }
}

pub fn put(c: &Connection, data: &[u8], mime: &str) -> Result<(BlobMeta, bool)> {
    crate::in_tx(c, |tx| {
        let mut w = BlobWriter::begin(tx)?;
        w.write(tx, data)?;
        w.finish(tx, mime)
    })
}

pub fn by_hash(c: &Connection, hash: &str) -> Result<Option<BlobMeta>> {
    Ok(c
        .query_row(
            "SELECT id, hash, size, mime FROM blobs WHERE hash = ?1 AND complete = 1",
            params![hash],
            |r| Ok(BlobMeta { id: r.get(0)?, hash: r.get(1)?, size: r.get(2)?, mime: r.get(3)? }),
        )
        .optional()?)
}

pub fn meta(c: &Connection, id: i64) -> Result<BlobMeta> {
    c.query_row(
        "SELECT id, hash, size, mime FROM blobs WHERE id = ?1 AND complete = 1",
        params![id],
        |r| Ok(BlobMeta { id: r.get(0)?, hash: r.get(1)?, size: r.get(2)?, mime: r.get(3)? }),
    )
    .optional()?
    .ok_or(VaultError::Missing)
}

pub fn read_all(c: &Connection, id: i64) -> Result<Vec<u8>> {
    let m = meta(c, id)?;
    let mut out = Vec::with_capacity(m.size as usize);
    let mut st = c.prepare_cached("SELECT data FROM blob_chunks WHERE blob_id = ?1 ORDER BY seq")?;
    let mut rows = st.query(params![id])?;
    while let Some(r) = rows.next()? {
        let d: Vec<u8> = r.get(0)?;
        out.extend_from_slice(&d);
    }
    Ok(out)
}

/// Reads bytes `[start, end)` without decrypting the rest of the file.
pub fn read_range(c: &Connection, id: i64, start: u64, end: u64) -> Result<Vec<u8>> {
    if end <= start {
        return Ok(Vec::new());
    }
    let first = (start / CHUNK as u64) as i64;
    let last = ((end - 1) / CHUNK as u64) as i64;
    let mut out = Vec::with_capacity((end - start) as usize);
    let mut st = c.prepare_cached(
        "SELECT seq, data FROM blob_chunks WHERE blob_id = ?1 AND seq BETWEEN ?2 AND ?3 ORDER BY seq",
    )?;
    let mut rows = st.query(params![id, first, last])?;
    while let Some(r) = rows.next()? {
        let seq: i64 = r.get(0)?;
        let d: Vec<u8> = r.get(1)?;
        let base = seq as u64 * CHUNK as u64;
        let lo = start.saturating_sub(base).min(d.len() as u64) as usize;
        let hi = (end - base).min(d.len() as u64) as usize;
        if lo < hi {
            out.extend_from_slice(&d[lo..hi]);
        }
    }
    Ok(out)
}

pub fn set_thumb(c: &Connection, blob_id: i64, mime: &str, data: &[u8]) -> Result<()> {
    c.execute(
        "INSERT OR REPLACE INTO thumbs(blob_id, mime, data) VALUES (?1, ?2, ?3)",
        params![blob_id, mime, data],
    )?;
    Ok(())
}

pub fn thumb(c: &Connection, blob_id: i64) -> Result<Option<(String, Vec<u8>)>> {
    Ok(c
        .query_row("SELECT mime, data FROM thumbs WHERE blob_id = ?1", params![blob_id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?)
}

/// Deletes blobs no document points at, plus abandoned partial uploads older than a day.
pub fn collect_garbage(c: &Connection) -> Result<usize> {
    let day_ago = now_ms() - 24 * 3600 * 1000;
    let grace = now_ms() - 10 * 60 * 1000;
    let n = c.execute(
        "DELETE FROM blobs WHERE
            (complete = 1 AND created_at < ?2 AND id NOT IN (SELECT blob_id FROM documents))
         OR (complete = 0 AND created_at < ?1)",
        params![day_ago, grace],
    )?;
    Ok(n)
}

pub fn total_bytes(c: &Connection) -> Result<i64> {
    Ok(c.query_row("SELECT COALESCE(SUM(size),0) FROM blobs WHERE complete = 1", [], |r| r.get(0))?)
}
