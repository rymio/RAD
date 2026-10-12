//! rad-server — owns the encrypted RAD vault and serves the RAD desktop apps.

use std::net::SocketAddr;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use rad_server::{Instance, ServerOptions};
use rad_vault::Vault;

#[derive(Parser)]
#[command(name = "rad-server", version, about = "RAD personal RAG server (encrypted single-file vault)")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
    #[command(flatten)]
    serve: ServeArgs,
}

#[derive(clap::Args, Clone)]
struct ServeArgs {
    /// Vault file (created on first use from the app or with `rad-server init`).
    #[arg(long, env = "RAD_VAULT", default_value = "./data/personal.radvault", global = true)]
    vault: PathBuf,
    /// Address to listen on.
    #[arg(long, env = "RAD_LISTEN", default_value = "0.0.0.0:8765")]
    listen: SocketAddr,
    /// Name shown in the apps.
    #[arg(long, env = "RAD_NAME", default_value = "RAD server")]
    name: String,
    /// Where paired-device records are kept (defaults to the vault's folder).
    #[arg(long, env = "RAD_STATE_DIR")]
    state_dir: Option<PathBuf>,
    /// Where encrypted backups are written (defaults to <vault folder>/backups).
    #[arg(long, env = "RAD_BACKUP_DIR")]
    backup_dir: Option<PathBuf>,
    /// TLS certificate (PEM). Use with --tls-key to serve HTTPS directly.
    #[arg(long, env = "RAD_TLS_CERT")]
    tls_cert: Option<PathBuf>,
    #[arg(long, env = "RAD_TLS_KEY")]
    tls_key: Option<PathBuf>,
    /// ffmpeg binary used for audio/video transcription.
    #[arg(long, env = "RAD_FFMPEG", default_value = "ffmpeg")]
    ffmpeg: String,
    /// Maximum upload size in MiB.
    #[arg(long, env = "RAD_MAX_UPLOAD_MB", default_value_t = 8192)]
    max_upload_mb: u64,
    /// Don't print a pairing code at startup.
    #[arg(long, env = "RAD_NO_PAIRING_CODE")]
    no_pairing_code: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the server (default).
    Serve,
    /// Create a new vault interactively and print its recovery kit.
    Init,
    /// Check the vault's encryption and integrity.
    Verify,
    /// Write an encrypted backup copy of the vault.
    Backup {
        #[arg(long)]
        to: PathBuf,
    },
}

fn read_pass(prompt: &str) -> anyhow::Result<String> {
    Ok(rpassword::prompt_password(prompt)?)
}

fn print_recovery_kit(key: &str) {
    println!();
    println!("  ╭────────────────────────── RAD RECOVERY KIT ──────────────────────────╮");
    println!("  │ Store this somewhere safe and offline (print it, password manager).  │");
    println!("  │ It opens your vault if you forget the passphrase. Anyone holding it  │");
    println!("  │ — plus a copy of the vault file — can read your data.                │");
    println!("  ╰───────────────────────────────────────────────────────────────────────╯");
    println!();
    println!("    {key}");
    println!();
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,tower_http=warn,rad_server=info".into()),
        )
        .init();
    let cli = Cli::parse();
    let args = cli.serve.clone();
    match cli.cmd.unwrap_or(Cmd::Serve) {
        Cmd::Init => {
            let p1 = read_pass("New vault passphrase: ")?;
            let p2 = read_pass("Repeat passphrase: ")?;
            anyhow::ensure!(p1 == p2, "passphrases do not match");
            let (v, recovery) = Vault::create(&args.vault, &p1)?;
            v.close();
            println!("Vault created: {}", args.vault.display());
            print_recovery_kit(&recovery);
            Ok(())
        }
        Cmd::Verify => {
            let p = read_pass("Vault passphrase: ")?;
            let v = Vault::open(&args.vault, &p)?;
            for m in v.integrity_check()? {
                println!("{m}");
            }
            Ok(())
        }
        Cmd::Backup { to } => {
            let p = read_pass("Vault passphrase: ")?;
            let v = Vault::open(&args.vault, &p)?;
            v.backup_to(&to)?;
            println!("Backup written to {}", to.display());
            Ok(())
        }
        Cmd::Serve => serve(args).await,
    }
}

async fn serve(args: ServeArgs) -> anyhow::Result<()> {
    let vault_dir = std::path::absolute(&args.vault)
        .unwrap_or(args.vault.clone())
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let opts = ServerOptions {
        vault: args.vault.clone(),
        name: args.name.clone(),
        state_dir: Some(args.state_dir.clone().unwrap_or(vault_dir)),
        backup_dir: args.backup_dir.clone(),
        ffmpeg: args.ffmpeg.clone(),
        max_upload_mb: args.max_upload_mb,
    };
    let inst = Instance::start(&opts).await?;

    println!();
    println!("  RAD server “{}” v{}", args.name, env!("CARGO_PKG_VERSION"));
    println!(
        "  vault:  {} ({})",
        inst.vault_path.display(),
        if Vault::exists(&inst.vault_path) { "locked" } else { "not created yet — create it from the RAD app" }
    );
    println!("  listen: {}://{}", if args.tls_cert.is_some() { "https" } else { "http" }, args.listen);
    if !args.no_pairing_code {
        let code = inst.new_pairing_code(15 * 60_000);
        println!();
        println!("  Pairing code (valid 15 min, single use):  {code}");
        println!("  Enter it in the RAD app → Connect to server. More codes: Settings → Devices.");
    }
    println!();

    match (args.tls_cert, args.tls_key) {
        (Some(cert), Some(key)) => rad_server::serve_tls(&inst, args.listen, cert, key).await,
        (None, None) => {
            rad_server::serve_http(&inst, args.listen, async {
                let _ = tokio::signal::ctrl_c().await;
                tracing::info!("shutting down — locking vault");
            })
            .await
        }
        _ => anyhow::bail!("--tls-cert and --tls-key must be given together"),
    }
}
