use anyhow::{Result, bail};

pub const DEFAULT_QUOTA_MB: u64 = 100;

pub fn quota_bytes(quota_mb: u64) -> u64 {
    quota_mb.saturating_mul(1024 * 1024)
}

/// Rejects a vfs whose declared total (ciphertext) size exceeds the
/// per-account quota. Called both client-side before publishing and
/// peer-side before a host accepts an incoming vfs pointer/chunks.
pub fn check_quota(total_bytes: u64, quota_mb: u64) -> Result<()> {
    let limit = quota_bytes(quota_mb);
    if total_bytes > limit {
        bail!(
            "vfs size {:.2} MB exceeds the {quota_mb} MB per-account quota",
            total_bytes as f64 / (1024.0 * 1024.0)
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enforces_quota() {
        assert!(check_quota(50 * 1024 * 1024, 100).is_ok());
        assert!(check_quota(150 * 1024 * 1024, 100).is_err());
    }
}
