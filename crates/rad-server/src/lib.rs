//! rad-server as a library: the standalone server binary and the RAD desktop app's
//! standalone mode both run exactly this code.

mod agent;
mod auth;
mod backup;
mod content;
mod devices;
mod error;
mod ingest;
mod media;
mod models;
mod rag;
mod routes;
mod sharing;
mod state;

pub mod embedded;
/// Which computers have a vault file open (see `sharing`).
pub use rad_vault::presence as vault_presence;

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::Context;

pub use state::Shared;

/// Unloads the built-in engine's models. Call once before the process exits.
pub fn shutdown_models() {
    rad_local::shutdown();
}

/// Everything needed to run a RAD server instance.
#[derive(Debug, Clone)]
pub struct ServerOptions {
    pub vault: PathBuf,
    pub name: String,
    /// Where paired-device records live. `None` keeps them in memory only (standalone mode).
    pub state_dir: Option<PathBuf>,
    pub backup_dir: Option<PathBuf>,
    pub ffmpeg: String,
    pub max_upload_mb: u64,
    /// Writable folder for downloaded GGUF models (defaults to `<vault folder>/models`).
    pub models_dir: Option<PathBuf>,
    /// Read-only folders searched for models first-hand, e.g. the copy shipped with the app.
    pub bundled_models: Vec<PathBuf>,
    /// Identifies this computer to others sharing the vault. Without an id, one is kept in
    /// `state_dir` (or derived from the host when there's none).
    pub machine_id: Option<String>,
    /// How this computer is named to others sharing the vault (defaults to the host name).
    pub machine_name: Option<String>,
}

/// A running (but not yet listening) server: state, background workers, media endpoint.
pub struct Instance {
    pub state: Shared,
    pub vault_path: PathBuf,
}

impl Instance {
    pub async fn start(opts: &ServerOptions) -> anyhow::Result<Instance> {
        let vault_path = std::path::absolute(&opts.vault).unwrap_or(opts.vault.clone());
        let vault_dir = vault_path.parent().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        std::fs::create_dir_all(&vault_dir).with_context(|| format!("creating {}", vault_dir.display()))?;
        let backup_dir = opts.backup_dir.clone().unwrap_or_else(|| vault_dir.join("backups"));
        let devices = match &opts.state_dir {
            Some(d) => devices::DeviceStore::load(d.join("rad-devices.json"))?,
            None => devices::DeviceStore::in_memory(),
        };

        let (media_listener, media_addr) = media::bind_loopback().await?;
        let (machine_id, machine_name) = sharing::machine_identity(opts.machine_id.clone(), opts.machine_name.clone(), opts.state_dir.as_deref());
        let cfg = state::Config {
            name: opts.name.clone(),
            vault_path: vault_path.clone(),
            backup_dir,
            ffmpeg: opts.ffmpeg.clone(),
            max_upload_bytes: opts.max_upload_mb * 1024 * 1024,
            media_addr: Some(media_addr),
            machine_id,
            machine_name,
        };
        let models_dir = opts.models_dir.clone().unwrap_or_else(|| vault_dir.join("models"));
        let library = rad_local::Library::new(models_dir, opts.bundled_models.clone());
        let state = state::AppState::new(cfg, devices, library.clone());
        media::serve_loopback(media_listener, state.clone());

        // Fetch the default embedding model now if this machine doesn't have it yet, so it's
        // ready (and works offline from then on) before the first document needs it.
        tokio::spawn(async move {
            if let Err(e) = library.ensure(&rad_local::DEFAULT_EMBEDDING).await {
                tracing::warn!("built-in embedding model unavailable (will retry when needed): {e}");
            }
        });

        let ff = media::check_ffmpeg(&state).await;
        state.ffmpeg_ok.store(ff, std::sync::atomic::Ordering::Relaxed);
        if !ff {
            tracing::warn!("ffmpeg not found ({}): audio/video transcription disabled", opts.ffmpeg);
        }

        tokio::spawn(ingest::worker_loop(state.clone()));
        backup::spawn_housekeeping(state.clone());
        Ok(Instance { state, vault_path })
    }

    pub fn router(&self) -> axum::Router {
        routes::router(self.state.clone())
    }

    pub fn new_pairing_code(&self, ttl_ms: i64) -> String {
        self.state.new_pairing_code(ttl_ms).0
    }

    /// Locks the vault (key leaves memory) and stops background work.
    pub fn stop(&self) {
        self.state.lock();
        self.state.stop();
    }
}

/// Plain-HTTP server until `shutdown` resolves; locks the vault on the way out.
pub async fn serve_http(inst: &Instance, listen: SocketAddr, shutdown: impl std::future::Future<Output = ()> + Send + 'static) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(listen).await.with_context(|| format!("binding {listen}"))?;
    serve_listener(inst, listener, shutdown).await
}

pub async fn serve_listener(inst: &Instance, listener: tokio::net::TcpListener, shutdown: impl std::future::Future<Output = ()> + Send + 'static) -> anyhow::Result<()> {
    axum::serve(listener, inst.router()).with_graceful_shutdown(shutdown).await?;
    inst.stop();
    Ok(())
}

pub async fn serve_tls(inst: &Instance, listen: SocketAddr, cert: PathBuf, key: PathBuf) -> anyhow::Result<()> {
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key).await.context("loading TLS certificate")?;
    let handle = axum_server::Handle::new();
    let h = handle.clone();
    let st = inst.state.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("shutting down — locking vault");
        st.lock();
        h.graceful_shutdown(Some(std::time::Duration::from_secs(5)));
    });
    axum_server::bind_rustls(listen, tls).handle(handle).serve(inst.router().into_make_service()).await?;
    inst.stop();
    Ok(())
}
