//! Standalone mode: a RAD server running inside the desktop app, reachable only on loopback
//! by the app itself (random port + an in-memory device token that never leaves the process).

use std::net::SocketAddr;
use std::path::PathBuf;

use crate::{Instance, ServerOptions};

pub struct Embedded {
    pub addr: SocketAddr,
    pub device_token: String,
    pub vault_path: PathBuf,
    inst: std::sync::Arc<Instance>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Embedded {
    /// `models_dir` is where downloaded models go; `bundled_models` are read-only folders
    /// (e.g. the app's resources) checked first.
    pub async fn start(vault: PathBuf, name: &str, ffmpeg: Option<String>, models_dir: PathBuf, bundled_models: Vec<PathBuf>) -> anyhow::Result<Embedded> {
        let opts = ServerOptions {
            vault,
            name: name.to_string(),
            state_dir: None,
            backup_dir: None,
            ffmpeg: ffmpeg.unwrap_or_else(default_ffmpeg),
            max_upload_mb: 16 * 1024,
            models_dir: Some(models_dir),
            bundled_models,
        };
        let inst = std::sync::Arc::new(Instance::start(&opts).await?);
        let (_device, device_token) = inst.state.devices.add("This computer", std::env::consts::OS)?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let i2 = inst.clone();
        let task = tokio::spawn(async move {
            if let Err(e) = crate::serve_listener(&i2, listener, async {
                let _ = rx.await;
            })
            .await
            {
                tracing::error!("embedded RAD server stopped: {e}");
            }
        });
        tracing::info!("standalone vault {} served on {addr}", inst.vault_path.display());
        Ok(Embedded { addr, device_token, vault_path: inst.vault_path.clone(), inst, shutdown: Some(tx), task: Some(task) })
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Locks the vault and stops the server. Waits briefly so the vault file is checkpointed.
    pub async fn stop(mut self) {
        let vault = self.inst.state.current_vault();
        self.inst.stop();
        // Background jobs (indexing, chat summaries) hold the vault until they finish; give them
        // a moment so the file is closed — and checkpointed into one file — before we return.
        if let Some(v) = vault {
            for _ in 0..50 {
                if std::sync::Arc::strong_count(&v) == 1 {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(t) = self.task.take() {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), t).await;
        }
    }
}

impl Drop for Embedded {
    fn drop(&mut self) {
        self.inst.stop();
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

/// ffmpeg is often installed outside the GUI app's PATH (e.g. Homebrew on macOS).
fn default_ffmpeg() -> String {
    for p in ["/opt/homebrew/bin/ffmpeg", "/usr/local/bin/ffmpeg", "/usr/bin/ffmpeg", "C:\\ffmpeg\\bin\\ffmpeg.exe"] {
        if std::path::Path::new(p).exists() {
            return p.to_string();
        }
    }
    "ffmpeg".into()
}
