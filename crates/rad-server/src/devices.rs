//! Paired devices live in a small JSON file next to the vault (not inside it) because they
//! must be checked while the vault is still locked. Only token hashes are stored.

use std::path::PathBuf;
use std::sync::Mutex;

use rad_api::Device;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Stored {
    device: Device,
    token_hash: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    devices: Vec<Stored>,
}

pub struct DeviceStore {
    /// `None` = in-memory only (standalone mode).
    path: Option<PathBuf>,
    inner: Mutex<File>,
}

pub fn hash_token(t: &str) -> String {
    blake3::hash(t.as_bytes()).to_hex().to_string()
}

pub fn random_hex(bytes: usize) -> String {
    use rand::RngCore;
    let mut b = vec![0u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut b);
    hex::encode(b)
}

impl DeviceStore {
    pub fn load(path: PathBuf) -> anyhow::Result<Self> {
        let inner = match std::fs::read(&path) {
            Ok(b) => serde_json::from_slice(&b)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => File::default(),
            Err(e) => return Err(e.into()),
        };
        Ok(DeviceStore { path: Some(path), inner: Mutex::new(inner) })
    }

    pub fn in_memory() -> Self {
        DeviceStore { path: None, inner: Mutex::new(File::default()) }
    }

    fn save(&self, f: &File) -> anyhow::Result<()> {
        let Some(path) = &self.path else { return Ok(()) };
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(f)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    /// Registers a device and returns its bearer token.
    pub fn add(&self, name: &str, os: &str) -> anyhow::Result<(Device, String)> {
        let token = format!("radd_{}", random_hex(32));
        let now = rad_vault::now_ms();
        let device = Device {
            id: random_hex(8),
            name: name.chars().take(80).collect(),
            os: os.chars().take(40).collect(),
            created_at: now,
            last_seen: now,
            revoked: false,
        };
        let mut f = self.inner.lock().unwrap();
        f.devices.push(Stored { device: device.clone(), token_hash: hash_token(&token) });
        self.save(&f)?;
        Ok((device, token))
    }

    pub fn authenticate(&self, token: &str) -> Option<Device> {
        let h = hash_token(token);
        let mut f = self.inner.lock().unwrap();
        let now = rad_vault::now_ms();
        let mut dirty = false;
        let found = f.devices.iter_mut().find(|d| !d.device.revoked && d.token_hash == h).map(|d| {
            if now - d.device.last_seen > 60_000 {
                d.device.last_seen = now;
                dirty = true;
            }
            d.device.clone()
        });
        if dirty {
            let _ = self.save(&f);
        }
        found
    }

    pub fn list(&self) -> Vec<Device> {
        self.inner.lock().unwrap().devices.iter().map(|d| d.device.clone()).collect()
    }

    pub fn revoke(&self, id: &str) -> anyhow::Result<bool> {
        let mut f = self.inner.lock().unwrap();
        let mut hit = false;
        for d in f.devices.iter_mut().filter(|d| d.device.id == id) {
            d.device.revoked = true;
            hit = true;
        }
        if hit {
            self.save(&f)?;
        }
        Ok(hit)
    }
}
