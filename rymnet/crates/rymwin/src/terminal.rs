use crate::app::{AppContext, RymApp};
use crate::browse;
use crate::rymscript;
use crate::theme;
use crate::winapp::WinAppWindow;
use eframe::egui;
use rustpython_vm::compiler;
use rymnet_core::NodeIdentity;
use rymnet_network::{PeerMessage, bytes_to_b64, send_request};
use rymnet_storage::{DataLayout, PeerConfig};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

/// The fourth desktop app, and the cheap option between the two the user
/// offered: a real PTY + VT100/ANSI terminal emulator would mean a new
/// heavy dependency, a parser to get right, and real cross-platform PTY
/// quirks across Linux/Windows/macOS. This instead reuses rymwin's own
/// command logic directly (vfs, browse, rymscript) behind a scrollback +
/// input-line widget -- the exact pattern `rymweb` already proved out
/// for the same "full rymsh functionality, no real terminal" problem in
/// a browser context.
///
/// Deliberately styled as a classic black-background terminal rather
/// than another gray Win-3.1 dialog -- the one app in this desktop that
/// should read as "a terminal" at a glance, not blend into the chrome.
pub struct TerminalApp {
    lines: Vec<String>,
    input: String,
    history: Vec<String>,
    history_index: usize,
    cwd: String,
    pending: Option<Pending>,
    run_session: Option<RunSession>,
    browse_rx: Option<mpsc::Receiver<Result<browse::FetchOutcome, String>>>,
    publish_rx: Option<mpsc::Receiver<Vec<String>>>,
    sendfile_rx: Option<mpsc::Receiver<Vec<String>>>,
    focus_requested: bool,
}

enum Pending {
    EditBody { path: String, lines: Vec<String> },
}

enum WorkerEvent {
    Print(String),
    InputRequest(String),
    Finished(Result<(), String>),
}

struct RunSession {
    events_rx: mpsc::Receiver<WorkerEvent>,
    input_tx: mpsc::Sender<String>,
    waiting_prompt: Option<String>,
}

impl TerminalApp {
    pub fn new() -> Self {
        Self {
            lines: vec![
                "Rymnet Terminal -- full rymsh-equivalent commands, no real TTY underneath.".to_string(),
                "Type `help` for what's available (and what isn't, yet).".to_string(),
                String::new(),
            ],
            input: String::new(),
            history: Vec::new(),
            history_index: 0,
            cwd: "/".to_string(),
            pending: None,
            run_session: None,
            browse_rx: None,
            publish_rx: None,
            sendfile_rx: None,
            focus_requested: false,
        }
    }

    fn instance_id(&self) -> usize {
        self as *const Self as usize
    }

    fn prompt(&self, app: &AppContext) -> String {
        format!("rymterm:{}@{}$ ", app.username, self.cwd)
    }

    fn resolve_path(&self, input: &str) -> String {
        let combined = if input.starts_with('/') {
            input.to_string()
        } else if self.cwd == "/" {
            format!("/{input}")
        } else {
            format!("{}/{input}", self.cwd)
        };
        normalize(&combined)
    }

    fn submit(&mut self, app: &AppContext, launch: &mut Vec<Box<dyn RymApp>>) {
        let line = std::mem::take(&mut self.input);

        if let Some(session) = &mut self.run_session
            && let Some(prompt) = session.waiting_prompt.take()
        {
            self.lines.push(format!("{prompt}{line}"));
            let _ = session.input_tx.send(line);
            return;
        }

        if let Some(pending) = self.pending.take() {
            let prompt = self.pending_prompt(&pending);
            self.lines.push(format!("{prompt}{line}"));
            let output = self.continue_pending(app, pending, &line);
            self.lines.extend(output);
            return;
        }

        let prompt = self.prompt(app);
        self.lines.push(format!("{prompt}{line}"));
        if !line.trim().is_empty() {
            self.history.push(line.clone());
        }
        self.history_index = self.history.len();
        let output = self.dispatch(app, launch, &line);
        self.lines.extend(output);
    }

    fn pending_prompt(&self, pending: &Pending) -> String {
        match pending {
            Pending::EditBody { .. } => "> ".to_string(),
        }
    }

    fn continue_pending(&mut self, app: &AppContext, pending: Pending, input: &str) -> Vec<String> {
        match pending {
            Pending::EditBody { path, mut lines } => {
                if input == "." {
                    return vec!["cancelled".to_string()];
                }
                if input.is_empty() {
                    let contents = lines.join("\n");
                    return match app.vfs.write_string(&path, &contents) {
                        Ok(()) => vec![format!("saved {path}")],
                        Err(err) => vec![format!("error: {err:#}")],
                    };
                }
                lines.push(input.to_string());
                self.pending = Some(Pending::EditBody { path, lines });
                Vec::new()
            }
        }
    }

    fn dispatch(&mut self, app: &AppContext, launch: &mut Vec<Box<dyn RymApp>>, line: &str) -> Vec<String> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }
        let tokens = match shell_words::split(trimmed) {
            Ok(tokens) => tokens,
            Err(err) => return vec![format!("parse error: {err}")],
        };
        let Some(command) = tokens.first().cloned() else {
            return Vec::new();
        };
        let args = &tokens[1..];

        match command.as_str() {
            "help" => help_text(),
            "clear" => {
                self.lines.clear();
                Vec::new()
            }
            "pwd" => vec![self.cwd.clone()],
            "cd" => {
                let target = self.resolve_path(args.first().map(String::as_str).unwrap_or("/"));
                match app.vfs.list_entries(&target) {
                    Ok(_) => {
                        self.cwd = target;
                        Vec::new()
                    }
                    Err(err) => vec![format!("cd: {err:#}")],
                }
            }
            "ls" => {
                let target = self.resolve_path(args.first().map(String::as_str).unwrap_or("."));
                match app.vfs.list_entries(&target) {
                    Ok(entries) if entries.is_empty() => vec!["(empty)".to_string()],
                    Ok(entries) => entries
                        .iter()
                        .map(|entry| {
                            if entry.is_dir {
                                format!("{}/", entry.name)
                            } else {
                                format!("{}  ({} bytes)", entry.name, entry.size)
                            }
                        })
                        .collect(),
                    Err(err) => vec![format!("ls: {err:#}")],
                }
            }
            "mkdir" => match args.first() {
                Some(path) => wrap(app.vfs.mkdir(&self.resolve_path(path))),
                None => vec!["usage: mkdir <dir>".to_string()],
            },
            "touch" => match args.first() {
                Some(path) => wrap(app.vfs.touch(&self.resolve_path(path))),
                None => vec!["usage: touch <file>".to_string()],
            },
            "rm" => {
                let recursive = args.iter().any(|a| a == "-r" || a == "-rf");
                match args.iter().find(|a| !a.starts_with('-')) {
                    Some(path) => wrap(app.vfs.rm(&self.resolve_path(path), recursive)),
                    None => vec!["usage: rm [-r] <path>".to_string()],
                }
            }
            "cp" => match (args.first(), args.get(1)) {
                (Some(src), Some(dst)) => wrap(app.vfs.cp(&self.resolve_path(src), &self.resolve_path(dst))),
                _ => vec!["usage: cp <src> <dst>".to_string()],
            },
            "mv" => match (args.first(), args.get(1)) {
                (Some(src), Some(dst)) => wrap(app.vfs.mv(&self.resolve_path(src), &self.resolve_path(dst))),
                _ => vec!["usage: mv <src> <dst>".to_string()],
            },
            "cat" => match args.first() {
                Some(path) => match app.vfs.read_to_string(&self.resolve_path(path)) {
                    Ok(text) => text.lines().map(str::to_string).collect(),
                    Err(err) => vec![format!("cat: {err:#}")],
                },
                None => vec!["usage: cat <file>".to_string()],
            },
            "edit" => match args.first() {
                Some(path) => {
                    let resolved = self.resolve_path(path);
                    let existing = app.vfs.read_to_string(&resolved).unwrap_or_default();
                    let mut out = vec![format!("editing {resolved} -- blank line to save, \".\" to cancel"), "----".to_string()];
                    out.extend(existing.lines().map(str::to_string));
                    out.push("----".to_string());
                    self.pending = Some(Pending::EditBody { path: resolved, lines: Vec::new() });
                    out
                }
                None => vec!["usage: edit <file>".to_string()],
            },
            "lynx" | "search" | "open" => {
                let path = match (command.as_str(), args.first()) {
                    ("search", Some(query_word)) => {
                        let query = args.join(" ");
                        let _ = query_word;
                        format!("{}/?q={}", browse::SEARCH_DOMAIN, browse::percent_encode(&query))
                    }
                    (_, Some(path)) => path.clone(),
                    (_, None) => return vec![format!("usage: {command} <domain.rym | query>")],
                };
                self.start_browse(app, path);
                vec!["fetching...".to_string()]
            }
            "run" => match args.first() {
                Some(name) => self.cmd_run(app, launch, name),
                None => vec!["usage: run <program>".to_string()],
            },
            "publish" => match args.first() {
                Some(domain) => {
                    self.start_publish(app, domain.clone());
                    vec![format!("Publishing {domain} from {}...", self.cwd)]
                }
                None => vec!["usage: publish <domain.rym>".to_string()],
            },
            "sendfile" => match (args.first(), args.get(1)) {
                (Some(path), Some(to)) => {
                    self.start_sendfile(app, path.clone(), to.clone());
                    vec![format!("Sending {path} to {to}...")]
                }
                _ => vec!["usage: sendfile <path> <user>".to_string()],
            },
            "whoami" => vec![format!("username: {}", app.username)],
            "status" => vec![
                format!("username: {}", app.username),
                format!("node id: {}", app.identity.node_id().map(|id| id.to_string()).unwrap_or_default()),
            ],
            other => vec![format!("unknown command: {other}. Type `help` for a list of commands.")],
        }
    }

    fn start_browse(&mut self, app: &AppContext, path: String) {
        let peers = match rymnet_storage::PeerConfig::load(app.layout.peers_path()) {
            Ok(peers) => peers,
            Err(err) => {
                self.lines.push(format!("error: {err:#}"));
                return;
            }
        };
        let (tx, rx) = mpsc::channel();
        let identity = app.identity.clone();
        app.runtime.spawn(async move {
            let result = browse::fetch_for_path(&identity, &peers, &path).await;
            let _ = tx.send(result);
        });
        self.browse_rx = Some(rx);
    }

    fn poll_browse(&mut self) {
        let Some(rx) = &self.browse_rx else { return };
        let Ok(result) = rx.try_recv() else { return };
        self.browse_rx = None;
        match result {
            Ok(browse::FetchOutcome::Page(page)) => {
                self.lines.push(format!("=== {} ===", page.title));
                self.lines.extend(wrap_text(&page.text, 88));
                if !page.links.is_empty() {
                    self.lines.push(String::new());
                    self.lines.push("Links:".to_string());
                    for link in &page.links {
                        self.lines.push(format!("  {link}"));
                    }
                }
            }
            Ok(browse::FetchOutcome::Search { query, hits }) => {
                self.lines.push(format!("Search: {query}"));
                if hits.is_empty() {
                    self.lines.push("No results.".to_string());
                } else {
                    for hit in &hits {
                        let title = if hit.title.is_empty() { hit.domain.clone() } else { hit.title.clone() };
                        self.lines.push(format!("  {title}  ({}{})", hit.domain, hit.path));
                    }
                }
            }
            Err(err) => self.lines.push(format!("error: {err}")),
        }
    }

    /// Claims (if not already owned), creates, uploads, publishes, and
    /// replicates the current directory as `domain` -- the same pipeline
    /// `rymsh`'s `publish` command chains, ported to direct library calls
    /// instead of shelling out to the `rym` binary (this app doesn't
    /// assume `rym` is even installed/on PATH, unlike a real shell). Runs
    /// on the background runtime since it's real network I/O; `dispatch`
    /// only prints the "Publishing..." placeholder synchronously.
    fn start_publish(&mut self, app: &AppContext, domain: String) {
        let source = app.vfs.real_path(&self.cwd);
        let layout = app.layout.clone();
        let (tx, rx) = mpsc::channel();
        app.runtime.spawn(async move {
            let output = run_publish(&layout, &domain, &source).await;
            let _ = tx.send(output);
        });
        self.publish_rx = Some(rx);
    }

    fn poll_publish(&mut self) {
        let Some(rx) = &self.publish_rx else { return };
        let Ok(output) = rx.try_recv() else { return };
        self.publish_rx = None;
        self.lines.extend(output);
    }

    /// Seals `path` (resolved against this terminal's own cwd, the same
    /// vfs sandbox every other command here uses) to `to`'s mailbox key,
    /// pushes the sealed chunk to peers, and sends the pointer as mail --
    /// see `mail::send_file`'s own doc comment for the full pipeline.
    fn start_sendfile(&mut self, app: &AppContext, path: String, to: String) {
        let real_path = app.vfs.real_path(&self.resolve_path(&path));
        let layout = app.layout.clone();
        let username = app.username.to_string();
        let identity = app.identity.clone();
        let (tx, rx) = mpsc::channel();
        app.runtime.spawn(async move {
            let output = match crate::mail::send_file(&layout, &username, &identity, &to, &real_path).await {
                Ok((delivered, total)) => vec![format!("Sent to {to}: delivered to {delivered}/{total} peer(s).")],
                Err(err) => vec![format!("error: {err:#}")],
            };
            let _ = tx.send(output);
        });
        self.sendfile_rx = Some(rx);
    }

    fn poll_sendfile(&mut self) {
        let Some(rx) = &self.sendfile_rx else { return };
        let Ok(output) = rx.try_recv() else { return };
        self.sendfile_rx = None;
        self.lines.extend(output);
    }

    fn poll_run_session(&mut self) {
        let Some(session) = &mut self.run_session else { return };
        loop {
            match session.events_rx.try_recv() {
                Ok(WorkerEvent::Print(text)) => self.lines.push(text),
                Ok(WorkerEvent::InputRequest(prompt)) => {
                    session.waiting_prompt = Some(prompt);
                    break;
                }
                Ok(WorkerEvent::Finished(result)) => {
                    if let Err(err) = result {
                        self.lines.push(format!("error: {err}"));
                    }
                    self.lines.push("(program finished)".to_string());
                    self.run_session = None;
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.run_session = None;
                    break;
                }
            }
        }
    }

    fn cmd_run(&mut self, app: &AppContext, launch: &mut Vec<Box<dyn RymApp>>, name: &str) -> Vec<String> {
        if self.run_session.is_some() {
            return vec!["a program is already running in this terminal -- open another terminal window to run something else concurrently".to_string()];
        }
        let run_name = if name.ends_with(".run") { name.to_string() } else { format!("{name}.run") };
        let run_path = format!("/Programs/{run_name}");
        if let Ok(source) = app.vfs.read_to_string(&run_path) {
            self.run_session = Some(spawn_run_session(source, app.vfs.root.clone()));
            return vec![format!("Running {run_path}...")];
        }

        let win_name = if let Some(stem) = name.strip_suffix(".run") {
            format!("{stem}.win")
        } else {
            format!("{name}.win")
        };
        let win_path = format!("/Programs/{win_name}");
        if let Ok(source) = app.vfs.read_to_string(&win_path) {
            launch.push(Box::new(WinAppWindow::new(win_name.clone(), &source, app.vfs.root.clone())));
            return vec![format!("Launched {win_name} as a desktop window (see the taskbar).")];
        }

        vec![format!("no such program: /Programs/{run_name} (use `ls /Programs` to see what's there)")]
    }

    fn current_prompt(&self, app: &AppContext) -> String {
        if let Some(session) = &self.run_session
            && let Some(prompt) = &session.waiting_prompt
        {
            return prompt.clone();
        }
        if let Some(pending) = &self.pending {
            return self.pending_prompt(pending);
        }
        self.prompt(app)
    }
}

impl RymApp for TerminalApp {
    fn title(&self) -> String {
        "Terminal".to_string()
    }

    fn show(&mut self, ctx: &egui::Context, app: &AppContext, launch: &mut Vec<Box<dyn RymApp>>) -> bool {
        self.poll_run_session();
        self.poll_browse();
        self.poll_publish();
        self.poll_sendfile();
        if self.run_session.is_some() || self.browse_rx.is_some() || self.publish_rx.is_some() || self.sendfile_rx.is_some() {
            ctx.request_repaint();
        }

        let mut open = true;
        theme::DecoratedWindow::new(egui::Id::new(("terminal_window", self.instance_id())), self.title())
            .icon("\u{1f4bb}")
            .default_size([620.0, 420.0])
            .show(ctx, &mut open, |ui| {
                egui::Frame::default()
                    .fill(egui::Color32::from_rgb(8, 8, 8))
                    .inner_margin(egui::Margin::same(6))
                    .show(ui, |ui| {
                        let text_color = egui::Color32::from_rgb(120, 230, 140);
                        ui.style_mut().visuals.override_text_color = Some(text_color);

                        // The input row used to be laid out after a
                        // ScrollArea whose height was `available_height()
                        // - 28.0` computed by hand -- fragile (the "28"
                        // was just a guess at this row's real height),
                        // and it meant the scrollback never actually
                        // participated in the window's own resize
                        // layout, just a stale snapshot of it from before
                        // the row below was drawn. `Panel::bottom` claims
                        // its real, measured height first, the same
                        // pattern `ircapp.rs`'s compose row and
                        // `mailapp.rs`'s toolbar already use successfully
                        // -- the scrollback below it then gets whatever
                        // is genuinely left, every frame, so dragging the
                        // window bigger or smaller actually changes how
                        // much scrollback is visible.
                        egui::Panel::bottom(egui::Id::new(("terminal_input_panel", self.instance_id())))
                            .resizable(false)
                            .frame(egui::Frame::NONE)
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.monospace(self.current_prompt(app));
                                    let response = ui.add(
                                        egui::TextEdit::singleline(&mut self.input)
                                            .desired_width(f32::INFINITY)
                                            .font(egui::TextStyle::Monospace)
                                            .text_color(text_color)
                                            .frame(egui::Frame::NONE),
                                    );
                                    if !self.focus_requested {
                                        response.request_focus();
                                        self.focus_requested = true;
                                    }
                                    if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                        self.submit(app, launch);
                                        response.request_focus();
                                    } else if response.has_focus() {
                                        if ui.input(|i| i.key_pressed(egui::Key::ArrowUp)) && self.history_index > 0 {
                                            self.history_index -= 1;
                                            self.input = self.history[self.history_index].clone();
                                        } else if ui.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                                            if self.history_index + 1 < self.history.len() {
                                                self.history_index += 1;
                                                self.input = self.history[self.history_index].clone();
                                            } else {
                                                self.history_index = self.history.len();
                                                self.input.clear();
                                            }
                                        }
                                    }
                                });
                            });

                        egui::ScrollArea::vertical()
                            .id_salt(("terminal_scroll", self.instance_id()))
                            .stick_to_bottom(true)
                            .show(ui, |ui| {
                                for line in &self.lines {
                                    ui.monospace(line);
                                }
                            });
                    });
            });
        open
    }
}

fn wrap(result: anyhow::Result<()>) -> Vec<String> {
    match result {
        Ok(()) => Vec::new(),
        Err(err) => vec![format!("error: {err:#}")],
    }
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if !current.is_empty() && current.len() + 1 + word.len() > width {
            lines.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", parts.join("/"))
    }
}

/// Claim/create/upload/publish/replicate, in that order, mirroring
/// `rymsh`'s own `cmd_publish` -- including its most important detail:
/// this uses the machine's own node identity (`rym init`'s identity,
/// loaded fresh from `layout.identity_path()`), not `AppContext.identity`
/// (the logged-in *account*'s identity). Domain/site ownership and the
/// username+password account system are two independent identities in
/// Rymnet, so publishing works the same whether or not anyone is logged
/// into this desktop session at all.
async fn run_publish(layout: &DataLayout, domain: &str, source: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let identity = match NodeIdentity::load(layout.identity_path()) {
        Ok(identity) => identity,
        Err(err) => return vec![format!("error: {err:#} -- run `rym init` first")],
    };

    // Claiming a domain you already own always "fails" the second time
    // (any valid existing claim blocks a re-claim) -- expected, not fatal.
    match rymnet_domain::claim_domain(layout.domain_claims_dir(), domain, &identity) {
        Ok(_) => out.push(format!("Claimed {domain}.")),
        Err(_) => out.push(format!("{domain} already claimed (continuing).")),
    }

    if let Err(err) = rymnet_site::create_site(domain, layout.site_root()) {
        out.push(format!("error: {err:#}"));
        return out;
    }
    let validation = match rymnet_site::upload_site(domain, layout.site_root(), source) {
        Ok(validation) => validation,
        Err(err) => {
            out.push(format!("error: {err:#}"));
            return out;
        }
    };
    if !validation.is_ok() {
        out.push("Upload rejected:".to_string());
        for rejection in &validation.rejected {
            out.push(format!("  {}: {}", rejection.path, rejection.reason));
        }
        return out;
    }

    let published = match rymnet_site::publish_site(domain, layout.site_root(), layout.chunks_root(), &identity) {
        Ok(published) => published,
        Err(err) => {
            out.push(format!("error: {err:#}"));
            return out;
        }
    };
    out.push(format!(
        "Published {domain} (version {}, {} file(s)).",
        published.manifest.site_version, published.files_hashed
    ));

    let peers = match PeerConfig::load(layout.peers_path()) {
        Ok(peers) => peers,
        Err(err) => {
            out.push(format!("warning: could not load peers: {err:#}"));
            return out;
        }
    };
    if peers.peers.is_empty() {
        out.push(
            "No peers configured -- saved locally only. Add one with `rym peer connect <ip>`, \
             then re-run `publish` to replicate."
                .to_string(),
        );
        return out;
    }
    let replicated = replicate_to_peers(layout, domain, &identity, &peers).await;
    out.push(format!("Replicated to {replicated}/{} peer(s).", peers.peers.len()));
    if replicated < peers.peers.len() {
        out.push("Some peers didn't accept it; re-run `publish` to retry.".to_string());
    }
    out.push(format!("Try: lynx {domain}"));
    out
}

/// Offers the just-published manifest and its chunks to every configured
/// peer, mirroring `rym`'s own `replicate_site` (the two intentionally
/// stay in lockstep since they're the same operation from two front
/// ends).
async fn replicate_to_peers(layout: &DataLayout, domain: &str, identity: &NodeIdentity, peers: &PeerConfig) -> usize {
    let Ok(Some(claim)) = rymnet_domain::find_claim(layout.domain_claims_dir(), domain) else {
        return 0;
    };
    let Ok(Some(manifest)) = rymnet_site::load_latest_manifest(domain, layout.site_root()) else {
        return 0;
    };
    let store = rymnet_storage::ChunkStore::new(layout.chunks_root());
    let mut chunks = Vec::new();
    for file in &manifest.files {
        let Ok(bytes) = store.get(&file.hash) else { return 0 };
        chunks.push(json!({"hash": file.hash, "bytes": bytes_to_b64(&bytes)}));
    }
    let payload = json!({"claim": claim, "manifest": manifest, "chunks": chunks});

    let mut accepted = 0;
    for peer in &peers.peers {
        let Ok(response) = send_request(peer, identity, PeerMessage::new("replica.offer", payload.clone())).await else {
            continue;
        };
        if response.message_type == "replica.status"
            && response.payload.get("status").and_then(|v| v.as_str()) == Some("complete")
        {
            accepted += 1;
        }
    }
    accepted
}

/// Runs a `.run` script on its own OS thread, entirely separate from
/// both the GUI thread and the tokio runtime: RustPython's VM types
/// (`Interpreter`, `Scope`, `PyObjectRef`, ...) are `Rc`-based and not
/// `Send`, so they can never cross a thread boundary -- but they don't
/// need to, since this thread creates and drives the whole VM itself and
/// only ever sends plain `String`/enum messages back out. This is what
/// makes real, blocking `input()` possible: when the script calls it,
/// the custom builtin sends a prompt over `events_tx` and then calls
/// `input_rx.recv()`, a genuine blocking wait (fine, since it's this
/// thread's own stack, not the GUI's) until the terminal sends back what
/// the user typed.
fn spawn_run_session(source: String, root: PathBuf) -> RunSession {
    let (events_tx, events_rx) = mpsc::channel::<WorkerEvent>();
    let (input_tx, input_rx) = mpsc::channel::<String>();

    std::thread::spawn(move || {
        let interpreter = rymscript::build_interpreter();
        let result = interpreter.enter(|vm| -> Result<(), String> {
            let scope = vm.new_scope_with_builtins();

            {
                let events_tx = events_tx.clone();
                scope
                    .globals
                    .set_item(
                        "print",
                        vm.new_function(
                            "print",
                            move |args: rustpython_vm::function::FuncArgs, vm: &rustpython_vm::VirtualMachine| -> rustpython_vm::PyResult<()> {
                                let mut parts = Vec::new();
                                for arg in &args.args {
                                    let py_str = arg.str(vm)?;
                                    let text: &str = py_str.as_ref();
                                    parts.push(text.to_string());
                                }
                                let _ = events_tx.send(WorkerEvent::Print(parts.join(" ")));
                                Ok(())
                            },
                        )
                        .into(),
                        vm,
                    )
                    .map_err(|exc| rymscript::format_exception(vm, exc))?;
            }
            {
                let events_tx = events_tx.clone();
                scope
                    .globals
                    .set_item(
                        "input",
                        vm.new_function(
                            "input",
                            move |prompt: rustpython_vm::function::OptionalArg<String>,
                                  vm: &rustpython_vm::VirtualMachine|
                                  -> rustpython_vm::PyResult<String> {
                                let prompt_text = prompt.into_option().unwrap_or_default();
                                let _ = events_tx.send(WorkerEvent::InputRequest(prompt_text));
                                input_rx
                                    .recv()
                                    .map_err(|_| vm.new_runtime_error("input cancelled".to_string()))
                            },
                        )
                        .into(),
                        vm,
                    )
                    .map_err(|exc| rymscript::format_exception(vm, exc))?;
            }

            rymscript::install_helpers(&scope, vm, root).map_err(|exc| rymscript::format_exception(vm, exc))?;

            let code = vm
                .compile(&source, compiler::Mode::Exec, "<rymscript>".to_string())
                .map_err(|err| err.to_string())?;
            vm.run_code_obj(code, scope.clone())
                .map_err(|exc| rymscript::format_exception(vm, exc))?;
            Ok(())
        });
        let _ = events_tx.send(WorkerEvent::Finished(result));
    });

    RunSession {
        events_rx,
        input_tx,
        waiting_prompt: None,
    }
}

fn help_text() -> Vec<String> {
    r#"Rymnet Terminal -- rymsh-equivalent commands against this desktop's
already-logged-in session (no separate login/logout here: use the
title bar's Log Out).

File commands (sandboxed to your virtual computer):
  ls [path]   cd <path>   pwd   mkdir <dir>   touch <file>
  rm [-r] <path>   cp <src> <dst>   mv <src> <dst>   cat <file>
  edit <file>          blank line to save, "." to cancel

Browsing (no local gateway; always resolved over the peer network):
  lynx <domain.rym>[/path]        search <query>

RymScript programs:
  run <name>           runs /Programs/<name>.run interactively (real
                       input() support) or launches /Programs/<name>.win
                       as a desktop window if no matching .win exists

Publishing:
  publish <domain.rym> claim/create/upload/publish/replicate the current
                       directory as a rymnet site, in one step (uses this
                       machine's own node identity, not whichever account
                       is logged in -- same as rymsh's `publish`)

Sending files (requires login):
  sendfile <path> <user>  seal a file from this vfs to one recipient's
                       mailbox key and send it as mail with an attachment
                       -- open it in the Mail app to save it back out

  whoami   status   clear   help

Not here yet: email (use the Mail app instead), mc (use the File Manager
app instead), and the `rym` CLI's remaining domain/site/peer/search/
index/node passthrough commands (use `rym` or `rymsh` directly for
those)."#
        .lines()
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn normalize_handles_dots_and_parent_dirs() {
        assert_eq!(normalize("/a/b/../c"), "/a/c");
        assert_eq!(normalize("/a/./b"), "/a/b");
        assert_eq!(normalize("/../.."), "/");
        assert_eq!(normalize("/"), "/");
        assert_eq!(normalize("/a//b"), "/a/b");
    }

    /// The highest-risk new code here is the interactive-`input()`
    /// threading protocol -- drives a real script through a real
    /// blocking mpsc round trip (prompt out, typed response in) exactly
    /// the way the GUI side does, without needing egui or a display.
    #[test]
    fn interactive_run_session_round_trips_input_and_print() {
        let root = std::env::temp_dir().join(format!("rymterm-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let source = r#"
print("hello")
name = input("name? ")
print("hi " + name)
"#
        .to_string();

        let session = spawn_run_session(source, root.clone());

        let mut events = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            match session.events_rx.recv_timeout(Duration::from_millis(200)) {
                Ok(WorkerEvent::Print(text)) => events.push(format!("print:{text}")),
                Ok(WorkerEvent::InputRequest(prompt)) => {
                    events.push(format!("input:{prompt}"));
                    session.input_tx.send("Ada".to_string()).unwrap();
                }
                Ok(WorkerEvent::Finished(result)) => {
                    events.push(format!("finished:{}", result.is_ok()));
                    break;
                }
                Err(_) if std::time::Instant::now() > deadline => panic!("timed out waiting for worker events"),
                Err(_) => continue,
            }
        }

        assert_eq!(
            events,
            vec![
                "print:hello".to_string(),
                "input:name? ".to_string(),
                "print:hi Ada".to_string(),
                "finished:true".to_string(),
            ]
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Runs the actual `checksum_tool.run` starter program (not a copy)
    /// through the same interactive session real `run <name>` uses, to
    /// prove the curated-stdlib example in RYMSCRIPT_SDK.md actually
    /// works end to end -- sha256, `random.Random()`, and `csv.writer`
    /// together, not just each module in isolation.
    #[test]
    fn checksum_tool_starter_program_runs_end_to_end() {
        let root = std::env::temp_dir().join(format!("rymterm-checksum-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("target.txt"), "hello world").unwrap();

        let source = include_str!("../../../starter_programs/checksum_tool.run").to_string();
        let session = spawn_run_session(source, root.clone());

        let mut prints = Vec::new();
        let finished: Result<(), String>;
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            match session.events_rx.recv_timeout(Duration::from_millis(200)) {
                Ok(WorkerEvent::Print(text)) => prints.push(text),
                Ok(WorkerEvent::InputRequest(_prompt)) => {
                    session.input_tx.send("target.txt".to_string()).unwrap();
                }
                Ok(WorkerEvent::Finished(result)) => {
                    finished = result;
                    break;
                }
                Err(_) if std::time::Instant::now() > deadline => panic!("timed out waiting for worker events"),
                Err(_) => continue,
            }
        }

        assert!(
            matches!(finished, Ok(())),
            "script should run to completion: prints={prints:?} finished={finished:?}"
        );
        assert!(
            prints.iter().any(|line| line == "sha256: b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"),
            "expected the known sha256 of \"hello world\", got: {prints:?}"
        );
        let confirmation_line = prints.iter().find(|line| line.starts_with("confirmation code: "));
        assert!(confirmation_line.is_some(), "expected a confirmation code line, got: {prints:?}");
        let code = confirmation_line.unwrap().trim_start_matches("confirmation code: ");
        assert_eq!(code.len(), 6, "confirmation code should be 6 digits, got {code:?}");
        assert!(code.chars().all(|c| c.is_ascii_digit()), "confirmation code should be all digits, got {code:?}");

        let log = std::fs::read_to_string(root.join("checksum_log.csv")).expect("checksum_log.csv should exist");
        assert!(log.starts_with("path,sha256,confirmation\r\n"), "expected a csv header, got: {log:?}");
        assert!(
            log.contains("target.txt,b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9,"),
            "expected a logged row for target.txt, got: {log:?}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
