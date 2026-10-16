use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct Hash(String);

impl Hash {
    pub fn new(hex: impl Into<String>) -> Self {
        Self(format!(
            "blake3:{}",
            hex.into().trim_start_matches("blake3:")
        ))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

pub fn hash_bytes(bytes: &[u8]) -> Hash {
    Hash::new(blake3::hash(bytes).to_hex().to_string())
}
