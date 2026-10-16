use crate::account_session::{self, AccountSession};
use crate::vfs::VirtualComputer;
use crate::{board, browser, editor, filemanager, irc, mail, python};
use anyhow::{Result, bail};
use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;
use rymnet_core::NodeIdentity;
use rymnet_storage::{AppConfig, DataLayout, PeerConfig};
use std::path::{Path, PathBuf};
use std::time::Duration;
use walkdir::WalkDir;

pub struct ShellContext {
    pub layout: DataLayout,
    pub vfs: VirtualComputer,
    pub gateway_addr: String,
    pub session: Option<AccountSession>,
}

enum ShellAction {
    Continue,
    Exit,
}

const RYM_SUBCOMMANDS: &[&str] = &[
    "domain", "site", "peer", "search", "index", "node", "version", "init", "open",
];

pub async fn run(mut ctx: ShellContext) -> Result<()> {
    let mut rl = DefaultEditor::new()?;
    let history_path = ctx.layout.root.join("rymsh_history.txt");
    let _ = rl.load_history(&history_path);

    loop {
        let prompt = match &ctx.session {
            Some(session) => format!("rymsh:{}@{}$ ", session.username, ctx.vfs.display_path()),
            None => format!("rymsh:{}$ ", ctx.vfs.display_path()),
        };
        match rl.readline(&prompt) {
            Ok(line) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let _ = rl.add_history_entry(trimmed);
                match dispatch(&mut ctx, trimmed).await {
                    Ok(ShellAction::Continue) => {}
                    Ok(ShellAction::Exit) => break,
                    Err(err) => println!("error: {err:#}"),
                }
            }
            Err(ReadlineError::Interrupted) => continue,
            Err(ReadlineError::Eof) => {
                let _ = auto_logout(&mut ctx).await;
                break;
            }
            Err(err) => {
                println!("readline error: {err}");
                break;
            }
        }
    }

    let _ = rl.save_history(&history_path);
    Ok(())
}

async fn dispatch(ctx: &mut ShellContext, line: &str) -> Result<ShellAction> {
    let tokens = shell_words::split(line)?;
    let Some(command) = tokens.first().cloned() else {
        return Ok(ShellAction::Continue);
    };
    let args = &tokens[1..];

    match command.as_str() {
        "help" => print_help(),
        "exit" | "quit" => {
            auto_logout(ctx).await;
            return Ok(ShellAction::Exit);
        }
        "clear" => print!("\x1B[2J\x1B[H"),
        "pwd" => println!("{}", ctx.vfs.pwd()),
        "ls" => {
            for entry in ctx.vfs.ls(args.first().map(String::as_str))? {
                println!("{entry}");
            }
        }
        "cd" => ctx
            .vfs
            .cd(args.first().map(String::as_str).unwrap_or("/"))?,
        "mkdir" => ctx.vfs.mkdir(require_arg(args, "mkdir <dir>")?)?,
        "touch" => ctx.vfs.touch(require_arg(args, "touch <file>")?)?,
        "rm" => {
            let recursive = args.iter().any(|arg| arg == "-r" || arg == "-rf");
            let target = args
                .iter()
                .find(|arg| !arg.starts_with('-'))
                .map(String::as_str)
                .ok_or_else(|| anyhow::anyhow!("usage: rm [-r] <path>"))?;
            ctx.vfs.rm(target, recursive)?;
        }
        "cp" => {
            let [src, dst] = require_two_args(args, "cp <src> <dst>")?;
            ctx.vfs.cp(src, dst)?;
        }
        "mv" => {
            let [src, dst] = require_two_args(args, "mv <src> <dst>")?;
            ctx.vfs.mv(src, dst)?;
        }
        "cat" => print!("{}", ctx.vfs.cat(require_arg(args, "cat <file>")?)?),
        "nano" | "edit" => {
            let target = require_arg(args, "nano <file>")?;
            let path = ctx.vfs.real_path(target);
            editor::edit(&path)?;
        }
        "status" => print_status(ctx).await,
        "time" => print_time(ctx).await,
        "df" => print_df(ctx),
        "whoami" => match &ctx.session {
            Some(session) => println!(
                "username: {}\npublic key: {}\nvfs version: {}",
                session.username, session.identity.public_key, session.vfs_version
            ),
            None => println!("not logged in. Use: login <username> <password>"),
        },
        "login" => {
            let [username, password] = require_two_args(args, "login <username> <password>")?;
            if ctx.session.is_some() {
                bail!("already logged in; run `logout` first");
            }
            println!("Deriving account keys and contacting peers...");
            let (session, summary) =
                account_session::login(&ctx.layout, username, password).await?;
            ctx.vfs = VirtualComputer::open(session.cache_dir.clone())?;
            println!(
                "Logged in as {} (peers reached: {})",
                session.username, summary.peers_reached
            );
            if summary.fresh_account {
                println!("No existing virtual computer found on the network; starting fresh.");
            } else {
                println!(
                    "Restored {} file(s) from the network.",
                    summary.files_restored
                );
            }
            ctx.session = Some(session);
        }
        "logout" => {
            if ctx.session.is_none() {
                bail!("not logged in");
            }
            auto_logout(ctx).await;
        }
        "sync" | "push" => {
            let session = ctx
                .session
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("log in first"))?;
            println!("Encrypting and pushing your virtual computer to peers...");
            let summary = account_session::push(&ctx.layout, session).await?;
            println!(
                "Pushed version {} ({} file(s), {:.2} MB) — accepted by {}/{} peers.",
                summary.vfs_version,
                summary.files_pushed,
                summary.total_bytes as f64 / (1024.0 * 1024.0),
                summary.peers_accepted,
                summary.peers_total
            );
        }
        "pull" => {
            let session = ctx
                .session
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("log in first"))?;
            let summary = account_session::pull(&ctx.layout, session).await?;
            println!(
                "Restored {} file(s) from the network.",
                summary.files_restored
            );
        }
        "lynx" | "browse" => {
            let target = require_arg(args, "lynx <domain.rym>[/path]")?;
            browser::browse(&ctx.gateway_addr, &ctx.layout, target).await?;
        }
        "search" => {
            if args.is_empty() {
                bail!("usage: search <query>");
            }
            let query = args.join(" ");
            let target = format!("search.rym/?q={}", browser::percent_encode(&query));
            browser::browse(&ctx.gateway_addr, &ctx.layout, &target).await?;
        }
        "emailto" => {
            let Some(session) = ctx.session.as_ref() else {
                bail!("log in first: login <username> <password>");
            };
            mail::compose_and_send(&ctx.layout, session, args.first().map(String::as_str)).await?;
        }
        "email" => {
            let Some(session) = ctx.session.as_ref() else {
                bail!("log in first: login <username> <password>");
            };
            mail::open_inbox(&ctx.layout, session, &ctx.vfs).await?;
        }
        "sendfile" => {
            let Some(session) = ctx.session.as_ref() else {
                bail!("log in first: login <username> <password>");
            };
            let [path, to] = require_two_args(args, "sendfile <path> <user>")?;
            let real_path = ctx.vfs.real_path(path);
            mail::send_file(&ctx.layout, session, &real_path, to).await?;
        }
        "irc" | "chat" => {
            let Some(session) = ctx.session.as_ref() else {
                bail!("log in first: login <username> <password>");
            };
            irc::run(&ctx.layout, session).await?;
        }
        "board" | "forum" => {
            let Some(session) = ctx.session.as_ref() else {
                bail!("log in first: login <username> <password>");
            };
            board::run(&ctx.layout, session).await?;
        }
        "mc" | "files" => {
            let Some(session) = ctx.session.as_ref() else {
                bail!("log in first: login <username> <password>");
            };
            let local_start = std::env::var("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/"));
            let account_root = session.cache_dir.clone();
            let username = session.username.clone();

            println!(
                "Opening file manager (computer on the left, rymnet:{username} on the right)..."
            );
            let right_dirty = filemanager::run(local_start, account_root, &username)?;
            if right_dirty {
                println!("Files in your rymnet account changed. Run `sync` to push them to peers.");
            }
            // deletes/renames in the file manager may have moved the vfs's
            // old working directory out from under it; reopen at home
            if let Some(session) = ctx.session.as_ref()
                && let Ok(vfs) = VirtualComputer::open(session.cache_dir.clone())
            {
                ctx.vfs = vfs;
            }
        }
        "python" | "py" => match args.first() {
            Some(script) => python::run_file(&ctx.vfs, script)?,
            None => python::repl(&ctx.vfs)?,
        },
        "run" => {
            let name = require_arg(args, "run <program>")?;
            cmd_run_program(ctx, name)?;
        }
        "publish" => {
            let domain = require_arg(args, "publish <domain.rym>")?;
            cmd_publish(ctx, domain).await;
        }
        "window" | "gui" => cmd_window(),
        other if RYM_SUBCOMMANDS.contains(&other) => passthrough_rym(ctx, &tokens).await?,
        other => println!("unknown command: {other}. Type `help` for a list of commands."),
    }

    Ok(ShellAction::Continue)
}

async fn auto_logout(ctx: &mut ShellContext) {
    let Some(session) = ctx.session.as_mut() else {
        return;
    };
    println!("Syncing your virtual computer before logging out...");
    match account_session::push(&ctx.layout, session).await {
        Ok(summary) => println!(
            "Pushed version {} ({} file(s)) to {}/{} peers.",
            summary.vfs_version, summary.files_pushed, summary.peers_accepted, summary.peers_total
        ),
        Err(err) => println!("warning: could not sync before logout: {err:#}"),
    }
    ctx.session = None;
    if let Ok(vfs) = VirtualComputer::open(ctx.layout.root.join("vfs")) {
        ctx.vfs = vfs;
    }
}

/// Publishes the current vfs directory as a rymnet site, chaining the
/// existing `rym domain claim` / `site create` / `site upload` / `site
/// publish` steps that already exist for exactly this purpose. Note this
/// uses the machine's own node identity (from `rym init`), not whichever
/// rymsh *account* you're logged into -- domain/site ownership and the
/// username+password account system are two independent identities in
/// rymnet, so this works the same whether or not you're logged in.
async fn cmd_publish(ctx: &ShellContext, domain: &str) {
    let source = ctx.vfs.real_path(".");
    println!("Publishing {domain} from {}", source.display());
    let source_str = source.display().to_string();

    // Claiming a domain you already own always "fails" the second time
    // (any valid existing claim blocks a re-claim) -- that's expected and
    // not fatal, so its result is intentionally ignored here.
    run_rym(ctx, &["domain", "claim", domain]).await;

    if !run_rym(ctx, &["site", "create", domain]).await {
        println!("Could not prepare a site directory for {domain}; aborting.");
        return;
    }
    if !run_rym(ctx, &["site", "upload", domain, &source_str]).await {
        println!(
            "Upload rejected; fix the reported issues above and run `publish {domain}` again."
        );
        return;
    }
    if !run_rym(ctx, &["site", "publish", domain]).await {
        println!("Publish failed; see the error above.");
        return;
    }
    println!(
        "Published. Try: lynx {domain}   (and `rym index site {domain}` to make it searchable)"
    );
}

async fn run_rym(ctx: &ShellContext, args: &[&str]) -> bool {
    match tokio::process::Command::new("rym")
        .arg("--data-dir")
        .arg(&ctx.layout.root)
        .args(args)
        .status()
        .await
    {
        Ok(status) => status.success(),
        Err(err) => {
            println!(
                "could not run `rym` ({err}); make sure it is installed and on PATH (see INSTALL_AND_CLI_GUIDE.md)"
            );
            false
        }
    }
}

async fn passthrough_rym(ctx: &ShellContext, tokens: &[String]) -> Result<()> {
    let status = tokio::process::Command::new("rym")
        .arg("--data-dir")
        .arg(&ctx.layout.root)
        .args(tokens)
        .status()
        .await;
    match status {
        Ok(_) => {}
        Err(err) => println!(
            "could not run `rym` ({err}); make sure it is installed and on PATH (see INSTALL_AND_CLI_GUIDE.md)"
        ),
    }
    Ok(())
}

async fn print_status(ctx: &ShellContext) {
    println!("Data dir: {}", ctx.layout.root.display());
    println!("Gateway: http://{}", ctx.gateway_addr);
    match tokio::time::timeout(
        Duration::from_millis(500),
        tokio::net::TcpStream::connect(&ctx.gateway_addr),
    )
    .await
    {
        Ok(Ok(_)) => println!("Daemon: reachable (rymnetd is running)"),
        _ => println!("Daemon: not reachable; start it with `rymnetd run`"),
    }
    match &ctx.session {
        Some(session) => {
            println!("Logged in as: {}", session.username);
            println!("Virtual computer: {}", session.cache_dir.display());
            println!("vfs version: {}", session.vfs_version);
        }
        None => println!("Logged in as: (none)"),
    }
}

/// Rymnet has no central time authority, so "network time" here means an
/// NTP-style estimate: sync against every configured peer, then take the
/// median offset (median rather than mean so one slow or lying peer can't
/// skew the result). Falls back to just the local clock when there's no
/// node identity or no peers to sync against.
async fn print_time(ctx: &ShellContext) {
    let local_now = rymnet_network::unix_millis_now();
    println!("Local time: {}", format_millis(local_now));

    let identity = match NodeIdentity::load(ctx.layout.identity_path()) {
        Ok(identity) => identity,
        Err(_) => {
            println!("Run `rym init` first to enable network time sync.");
            return;
        }
    };
    let peers = match PeerConfig::load(ctx.layout.peers_path()) {
        Ok(peers) => peers,
        Err(err) => {
            println!("could not load peers: {err:#}");
            return;
        }
    };
    if peers.peers.is_empty() {
        println!("No peers configured; nothing to sync against (see `rym peer add`).");
        return;
    }

    let mut offsets = Vec::new();
    for peer in &peers.peers {
        match rymnet_network::time_sync(peer, &identity).await {
            Ok(sample) => {
                println!(
                    "{:<20} peer time {}  offset {:+}ms  rtt {}ms",
                    peer.name,
                    format_millis(sample.peer_time_ms),
                    sample.offset_ms,
                    sample.round_trip_ms,
                );
                offsets.push(sample.offset_ms);
            }
            Err(err) => println!("{:<20} unreachable: {err:#}", peer.name),
        }
    }

    if offsets.is_empty() {
        println!("No peers reachable; cannot compute network time.");
        return;
    }
    offsets.sort_unstable();
    let median_offset = offsets[offsets.len() / 2];
    println!();
    println!(
        "Network time (median of {} peer(s)): {}",
        offsets.len(),
        format_millis(local_now + median_offset)
    );
    println!("Clock offset from local system time: {median_offset:+}ms");
}

fn format_millis(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| ms.to_string())
}

/// Shows how much of the account's storage quota is used, the same way
/// `df` shows disk usage. Quota only applies to logged-in accounts (it is
/// what peers enforce when hosting your encrypted vfs); the anonymous local
/// vfs is just scratch space on this machine with no quota.
fn print_df(ctx: &ShellContext) {
    let used = dir_size(&ctx.vfs.root);
    println!(
        "{:<26} {:>10} {:>10} {:>10} {:>6}",
        "VIRTUAL COMPUTER", "SIZE", "USED", "AVAIL", "USE%"
    );
    match &ctx.session {
        Some(session) => {
            let account = AppConfig::load(ctx.layout.config_path())
                .map(|config| config.account)
                .unwrap_or_default();
            if account.is_unlimited(&session.username) {
                println!(
                    "{:<26} {:>10} {:>10} {:>10} {:>6}",
                    format!("rymnet:{}", session.username),
                    "unlimited",
                    human_bytes(used),
                    "unlimited",
                    "0.0%"
                );
                println!();
                println!(
                    "{} is exempt from the storage quota on this node (see [account].unlimited_usernames \
                     in config.toml).",
                    session.username
                );
            } else {
                let quota_bytes = rymnet_account::quota_bytes(account.quota_mb);
                let avail = quota_bytes.saturating_sub(used);
                let pct = if quota_bytes == 0 {
                    0.0
                } else {
                    used as f64 / quota_bytes as f64 * 100.0
                };
                println!(
                    "{:<26} {:>10} {:>10} {:>10} {:>5.1}%",
                    format!("rymnet:{}", session.username),
                    human_bytes(quota_bytes),
                    human_bytes(used),
                    human_bytes(avail),
                    pct
                );
                println!();
                println!(
                    "USED reflects your current local files. Actual usage after `sync` is\n\
                     slightly higher (~16 bytes/file of encryption overhead). The {} MB\n\
                     quota is enforced both when you push and by every peer hosting your account.",
                    account.quota_mb
                );
            }
        }
        None => {
            println!(
                "{:<26} {:>10} {:>10} {:>10} {:>6}",
                "local (not logged in)",
                "--",
                human_bytes(used),
                "--",
                "--"
            );
            println!();
            println!("Log in to see your account quota: login <username> <password>");
        }
    }
}

fn dir_size(path: &Path) -> u64 {
    WalkDir::new(path)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
        .filter_map(|entry| entry.metadata().ok())
        .map(|metadata| metadata.len())
        .sum()
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

/// Runs `/Programs/<name>.run` -- a `*.win` desktop app named the same
/// way gets a clear redirect instead of a confusing file-not-found, since
/// that's the single most likely mix-up (running the wrong extension).
fn cmd_run_program(ctx: &ShellContext, name: &str) -> Result<()> {
    let run_name = if name.ends_with(".run") { name.to_string() } else { format!("{name}.run") };
    let run_path = format!("/Programs/{run_name}");
    if ctx.vfs.real_path(&run_path).exists() {
        return python::run_file(&ctx.vfs, &run_path);
    }

    let win_name = if let Some(stem) = name.strip_suffix(".run") {
        format!("{stem}.win")
    } else {
        format!("{name}.win")
    };
    let win_path = format!("/Programs/{win_name}");
    if ctx.vfs.real_path(&win_path).exists() {
        bail!(
            "{win_name} is a desktop app, not a terminal program -- open it from Rymcode, \
             or type `window` to launch the graphical desktop"
        );
    }

    bail!("no such program: /Programs/{run_name} (use `ls /Programs` to see what's there)");
}

fn cmd_window() {
    if !graphics_available() {
        println!(
            "no graphics available in this session (no $DISPLAY or $WAYLAND_DISPLAY \
             detected) -- this looks like a headless/SSH session with no X forwarding; \
             use the terminal commands here instead"
        );
        return;
    }

    let mut command = std::process::Command::new("rymwin");
    // Fully detach: a GUI app's AppKit/win32 event loop must survive rymsh
    // exiting (e.g. right after `window` the user types `exit`). Without
    // this, the child stays in rymsh's process group/session and inherits
    // its stdio pipes, and on macOS that reliably crashed the freshly
    // spawned window mid-initialization when the parent shell exited.
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }

    match command.spawn() {
        Ok(_) => println!("opening Rymnet Desktop..."),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            println!(
                "rymwin isn't installed or not on PATH -- install it with: \
                 cargo install --path crates/rymwin --force"
            );
        }
        Err(err) => println!("failed to launch rymwin: {err:#}"),
    }
}

/// Linux gives a reliable signal for "is a display actually reachable"
/// via these env vars; macOS/Windows don't have an equivalent one-shot
/// check, so there we just let the spawn attempt itself be the real test.
fn graphics_available() -> bool {
    if cfg!(target_os = "linux") {
        std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
    } else {
        true
    }
}

fn require_arg<'a>(args: &'a [String], usage: &str) -> Result<&'a str> {
    args.first()
        .map(String::as_str)
        .ok_or_else(|| anyhow::anyhow!("usage: {usage}"))
}

fn require_two_args<'a>(args: &'a [String], usage: &str) -> Result<[&'a str; 2]> {
    if args.len() < 2 {
        bail!("usage: {usage}");
    }
    Ok([args[0].as_str(), args[1].as_str()])
}

fn print_help() {
    println!(
        r#"rymsh — interactive shell for your Rymnet virtual computer

File commands (local to this virtual computer):
  ls [path]            list files
  cd <path>             change directory
  pwd                    print working directory
  mkdir <dir>            make a directory
  touch <file>           create an empty file
  rm [-r] <path>         remove a file or (with -r) a directory
  cp <src> <dst>         copy a file or directory
  mv <src> <dst>         move/rename a file or directory
  cat <file>             print a file's contents
  nano <file>            edit a file (small nano-style editor)
  clear                  clear the screen

Account (password-derived identity, synced over the rymnet peer network):
  login <user> <pass>    derive your identity and pull your files from peers
  logout                 push your files to peers and end the session
  sync | push            push local changes to peers now
  pull                   pull the latest version from peers now
  whoami                 show the logged-in account

Browsing (rymnet sites are HTML/CSS only, like the real web of 1994):
  lynx <domain.rym>[/path]   browse a published rymnet site
  search <query>          search the whole network and browse the results
                         (same as `lynx search.rym`, then typing your query;
                          inside lynx, typing text with no ".rym" in it also
                          searches -- text that looks like a domain still
                          navigates there)

Email (requires login; no classic SMTP/mail server -- see the guide for how):
  emailto [user]         compose: prompts for To (if not given), Cc, Bcc,
                         Subject, and a blank-line-terminated body, then
                         delivers it to every configured peer
  email                  fetch your inbox and open a numbered list; enter a
                         number to read a message, then r to reply, f to
                         forward, s to save an attachment, or q to go back
  sendfile <path> <user> seal a file from your virtual computer to one
                         recipient (their mailbox key, same as email) and
                         send it -- appears as a normal mail message with
                         an attachment; peers relaying it only ever see
                         ciphertext

IRC (requires login; channels are signed, public, and relayed the same
     way mail is -- no node ever needs to know where anyone else is
     connected, only where its own configured peers are):
  irc | chat              open a full-screen chat session
                         /join #channel   switch/join a channel
                         /msg <user> <text>   send a direct message (mail)
                         /quit or Ctrl-C  leave
                         plain text sends to the joined channel; messages
                         from others typically appear within a few
                         seconds (polled, not pushed)

Board (requires login; same signed/public/relayed model as IRC, a
       simple Reddit-style forum -- named boards, text posts with an
       optional link, flat one-level comment replies, no voting):
  board | forum           open a full-screen board session
                         /board <name>    switch/join a board
                         Up/Down + Enter  select and open a post
                         /new <title> | <body>   post to the active board
                         plain text (inside a thread) posts a comment
                         /back            return to the post list
                         /quit or Ctrl-C  leave

File manager (requires login; computer on the left, rymnet account on the right):
  mc | files             open a two-pane file manager to copy files between
                         your real machine and your encrypted rymnet account
                         (Tab switch pane, Enter open, c copy, m mkdir,
                         n new file, r rename, d delete, e edit, q quit)

Python (a small embedded interpreter, sandboxed to this virtual computer):
  python | py             start an interactive Python REPL
  python <file.py>        run a script from this virtual computer
                         (read_file/write_file/append_file/list_dir/exists/
                          remove_file/mkdir are the only file access; no
                          real filesystem, network, or process access exists)

RymScript programs (see RYMSCRIPT_SDK.md for the full reference):
  run <name>               run Programs/<name>.run (terminal program) --
                          same sandboxed file API as `python`, plus
                          print()/input() for terminal I/O. *.win desktop
                          apps in Programs/ are launched from Rymcode or
                          File Manager instead (see `window`), not `run`.

Network (passed through to the `rym` CLI):
  domain ...  site ...  peer ...  search ...  index ...  node ...
  publish <domain.rym>   claim/create/upload/publish the current directory
                         as a rymnet site in one step

Graphical desktop (requires a display; opens a separate window):
  window | gui            launch rymwin, the graphical Rymnet Desktop
                         (own login, own session -- see the guide for why)

  status                 show daemon/gateway/account status
  time                    show local time and network-synced time (median
                         clock offset across your configured peers)
  df                     show how much of your account storage quota is used
  exit | quit             leave rymsh (auto-syncs if logged in)
"#
    );
}
