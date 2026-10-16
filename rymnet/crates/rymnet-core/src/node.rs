use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct NodeId(String);

impl NodeId {
    pub fn from_public_key(public_key: &[u8]) -> Self {
        let digest = blake3::hash(public_key);
        let short = &digest.as_bytes()[..16];
        Self(format!("rym1{}", hex::encode(short)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

pub fn public_key_display(public_key: &[u8]) -> String {
    format!("rympub1{}", STANDARD.encode(public_key))
}
