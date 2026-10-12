//! The device token lives in the OS keychain (macOS Keychain, Windows Credential Manager,
//! Secret Service on Linux). If no keychain is available we fall back to a 0600 file.

use std::path::Path;

const SERVICE: &str = "com.rymtech.rad";

fn account(server: &str) -> String {
    format!("device-token:{}", server.trim_end_matches('/'))
}

fn fallback_path(dir: &Path, server: &str) -> std::path::PathBuf {
    dir.join(format!("token-{}.secret", blake3::hash(server.as_bytes()).to_hex()[..16].to_string()))
}

pub fn get(dir: &Path, server: &str) -> Option<String> {
    if let Ok(e) = keyring::Entry::new(SERVICE, &account(server)) {
        if let Ok(t) = e.get_password() {
            return Some(t);
        }
    }
    std::fs::read_to_string(fallback_path(dir, server)).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

pub fn set(dir: &Path, server: &str, token: &str) -> anyhow::Result<()> {
    if let Ok(e) = keyring::Entry::new(SERVICE, &account(server)) {
        if e.set_password(token).is_ok() && e.get_password().ok().as_deref() == Some(token) {
            let _ = std::fs::remove_file(fallback_path(dir, server));
            return Ok(());
        }
    }
    tracing::warn!("OS keychain unavailable; storing the device token in a private file");
    std::fs::create_dir_all(dir)?;
    let p = fallback_path(dir, server);
    std::fs::write(&p, token)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

pub fn delete(dir: &Path, server: &str) {
    if let Ok(e) = keyring::Entry::new(SERVICE, &account(server)) {
        let _ = e.delete_credential();
    }
    let _ = std::fs::remove_file(fallback_path(dir, server));
}
