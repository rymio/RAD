use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct ChunkStore {
    root: PathBuf,
}

impl ChunkStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn put(&self, bytes: &[u8]) -> Result<String> {
        let hash = format!("blake3:{}", blake3::hash(bytes).to_hex());
        let path = self.path_for_hash(&hash)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        if !path.exists() {
            fs::write(&path, bytes).with_context(|| format!("write chunk {}", path.display()))?;
        }
        Ok(hash)
    }

    pub fn get(&self, hash: &str) -> Result<Vec<u8>> {
        let path = self.path_for_hash(hash)?;
        fs::read(&path).with_context(|| format!("read chunk {}", path.display()))
    }

    pub fn exists(&self, hash: &str) -> Result<bool> {
        Ok(self.path_for_hash(hash)?.exists())
    }

    pub fn verify(&self, hash: &str, bytes: &[u8]) -> bool {
        format!("blake3:{}", blake3::hash(bytes).to_hex()) == hash
    }

    pub fn path_for_hash(&self, hash: &str) -> Result<PathBuf> {
        let hex = hash
            .strip_prefix("blake3:")
            .ok_or_else(|| anyhow::anyhow!("chunk hash must start with blake3:"))?;
        if hex.len() < 4 || !hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
            anyhow::bail!("invalid BLAKE3 hash");
        }
        Ok(self.root.join(&hex[0..2]).join(&hex[2..4]).join(hex))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_get_exists_and_verify_chunk() {
        let dir =
            std::env::temp_dir().join(format!("rymnet-chunk-test-{}", blake3::hash(b"chunk")));
        let _ = fs::remove_dir_all(&dir);
        let store = ChunkStore::new(&dir);
        let bytes = b"hello rymnet";
        let hash = store.put(bytes).expect("put");

        assert!(store.exists(&hash).expect("exists"));
        assert_eq!(store.get(&hash).expect("get"), bytes);
        assert!(store.verify(&hash, bytes));
        assert!(!store.verify(&hash, b"tampered"));

        let _ = fs::remove_dir_all(&dir);
    }
}
