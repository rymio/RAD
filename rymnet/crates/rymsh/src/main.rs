mod account_session;
mod board;
mod browser;
mod editor;
mod filemanager;
mod irc;
mod mail;
mod python;
mod shell;
mod vfs;

use anyhow::Result;
use clap::Parser;
use rymnet_storage::{AppConfig, DataLayout};
use shell::ShellContext;
use std::time::Duration;
use vfs::VirtualComputer;

#[derive(Debug, Parser)]
#[command(
    name = "rymsh",
    version,
    about = "Interactive shell for a Rymnet virtual computer"
)]
struct Cli {
    #[arg(long, value_name = "DIR")]
    data_dir: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_target(false)
        .init();

    let cli = Cli::parse();
    let layout = match cli.data_dir.as_deref() {
        Some(dir) => DataLayout::from_root(rymnet_storage::layout::expand_tilde(dir)?),
        None => DataLayout::default()?,
    };
    layout.ensure_all()?;

    let gateway_addr = AppConfig::load(layout.config_path())
        .map(|config| config.gateway.listen_addr)
        .unwrap_or_else(|_| "127.0.0.1:7447".to_string());

    println!("rymnet interactive shell (rymsh)");
    println!("data dir: {}", layout.root.display());
    match tokio::time::timeout(
        Duration::from_millis(500),
        tokio::net::TcpStream::connect(&gateway_addr),
    )
    .await
    {
        Ok(Ok(_)) => println!("connected to local rymnet daemon at {gateway_addr}"),
        _ => println!(
            "no local rymnet daemon detected at {gateway_addr}; browsing, search, and account \
             sync will talk directly to your configured peers instead (see `rym peer list`)"
        ),
    }
    println!("type `help` for a list of commands\n");

    let vfs = VirtualComputer::open(layout.root.join("vfs"))?;
    let ctx = ShellContext {
        layout,
        vfs,
        gateway_addr,
        session: None,
    };

    shell::run(ctx).await
}
