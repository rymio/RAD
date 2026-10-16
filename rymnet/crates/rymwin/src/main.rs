mod account;
mod app;
mod board;
mod boardapp;
mod browse;
mod browserapp;
mod designer;
mod embed;
mod filemanager;
mod irc;
mod ircapp;
mod mail;
mod mailapp;
mod paintapp;
mod pdf;
mod pdfreaderapp;
mod rymcode;
mod rymscript;
mod taskbar;
mod terminal;
mod theme;
mod vfs;
mod winapp;
mod writerapp;

use account::LoggedInAccount;
use anyhow::Result;
use app::{AppContext, RymApp};
use eframe::egui;
use rymnet_core::NodeIdentity;
use rymnet_storage::DataLayout;
use std::sync::mpsc;
use vfs::VirtualComputer;

fn main() -> eframe::Result<()> {
    // Inside RAD: no window of its own; RAD shows the frames in its RYMWIN view.
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--embed") {
        let mut app = DesktopApp::new();
        let setup = |ctx: &egui::Context| {
            ctx.set_visuals(theme::retro_visuals());
            egui_extras::install_image_loaders(ctx);
        };
        if let Err(err) = embed::run(embed::Options::from_args(&args), setup, |ui| app.draw(ui)) {
            eprintln!("rymwin --embed: {err:#}");
            std::process::exit(1);
        }
        return Ok(());
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1024.0, 768.0])
            .with_min_inner_size([1024.0, 768.0])
            // Was `false`: the whole desktop was locked to exactly
            // one fixed size at the OS level, which meant no app window
            // inside it -- Terminal included -- could ever be dragged
            // past that ceiling either, since there was no bigger canvas
            // to grow into. `with_min_inner_size` above keeps the retro
            // 1024x768 starting size as a floor, not a cage.
            .with_resizable(true)
            .with_title("Rymnet Desktop"),
        ..Default::default()
    };

    eframe::run_native(
        "Rymnet Desktop",
        options,
        Box::new(|cc| {
            cc.egui_ctx.set_visuals(theme::retro_visuals());
            egui_extras::install_image_loaders(&cc.egui_ctx);
            Ok(Box::new(DesktopApp::new()))
        }),
    )
}

enum Screen {
    Login(LoginState),
    Desktop(Box<DesktopState>),
}

#[derive(Default)]
struct LoginState {
    username: String,
    password: String,
    status: Option<String>,
    status_is_error: bool,
    busy: bool,
    rx: Option<mpsc::Receiver<Result<LoggedInAccount>>>,
}

struct DesktopState {
    account: LoggedInAccount,
    vfs: VirtualComputer,
    apps: Vec<Box<dyn RymApp>>,
    logging_out: bool,
    rx: Option<mpsc::Receiver<Result<account::PushSummary>>>,
    status: Option<String>,
    status_is_error: bool,
}

struct DesktopApp {
    runtime: tokio::runtime::Runtime,
    layout: DataLayout,
    identity: NodeIdentity,
    screen: Screen,
}

impl DesktopApp {
    fn new() -> Self {
        // Same reasoning as rymweb: the default worker stack is too small
        // for the async call depth of a login (identity derive -> claim
        // announce -> vfs pull -> per-peer chunk fetch), so the runtime is
        // built by hand instead of via #[tokio::main].
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_stack_size(16 * 1024 * 1024)
            .build()
            .expect("build tokio runtime");
        let layout = DataLayout::default().expect("resolve ~/.rymnet data directory");
        // A node identity independent of any logged-in account -- browsing
        // works whether or not you're logged in, the same way rymweb keeps
        // a `server_identity` for anonymous peer requests.
        let identity = NodeIdentity::load(layout.identity_path())
            .expect("load node identity; run `rym init` first");
        Self {
            runtime,
            layout,
            identity,
            screen: Screen::Login(LoginState::default()),
        }
    }
}

impl eframe::App for DesktopApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }
}

impl DesktopApp {
    /// One frame of the desktop: the same in the native window and embedded in RAD.
    fn draw(&mut self, ui: &mut egui::Ui) {
        match &mut self.screen {
            Screen::Login(_) => {
                let next = show_login(ui, &self.runtime, &self.layout, &mut self.screen);
                if let Some(next) = next {
                    self.screen = next;
                }
            }
            Screen::Desktop(_) => {
                let next = show_desktop(ui, &self.runtime, &self.layout, &self.identity, &mut self.screen);
                if let Some(next) = next {
                    self.screen = next;
                }
            }
        }
    }
}

fn show_login(
    ui: &mut egui::Ui,
    runtime: &tokio::runtime::Runtime,
    layout: &DataLayout,
    screen: &mut Screen,
) -> Option<Screen> {
    let Screen::Login(state) = screen else {
        return None;
    };

    if let Some(rx) = &state.rx {
        if let Ok(result) = rx.try_recv() {
            state.busy = false;
            state.rx = None;
            match result {
                Ok(account) => {
                    let vfs = match VirtualComputer::open(account.cache_dir.clone()) {
                        Ok(vfs) => vfs,
                        Err(err) => {
                            state.status = Some(format!("Login succeeded but failed to open virtual computer: {err:#}"));
                            state.status_is_error = true;
                            return None;
                        }
                    };
                    let status = Some(if account.fresh_account {
                        format!(
                            "Welcome! No existing virtual computer found on the network yet ({} peer(s) reached).",
                            account.peers_reached
                        )
                    } else {
                        format!(
                            "Restored {} file(s), vfs version {} ({} peer(s) reached).",
                            account.files_restored, account.vfs_version, account.peers_reached
                        )
                    });
                    return Some(Screen::Desktop(Box::new(DesktopState {
                        account,
                        vfs,
                        apps: Vec::new(),
                        logging_out: false,
                        rx: None,
                        status,
                        status_is_error: false,
                    })));
                }
                Err(err) => {
                    state.status = Some(format!("Login failed: {err:#}"));
                    state.status_is_error = true;
                }
            }
        } else {
            ui.ctx().request_repaint();
        }
    }

    taskbar::show(ui, false);

    let (hovering, dropped) = ui.ctx().input(|i| (!i.raw.hovered_files.is_empty(), !i.raw.dropped_files.is_empty()));
    if hovering {
        drop_overlay(ui.ctx(), "Log in first, then drop files to copy them in");
    }
    if dropped {
        state.status = Some("Log in first, then drop the files again.".into());
        state.status_is_error = true;
    }

    egui::CentralPanel::default()
        .frame(egui::Frame::default().fill(theme::DESKTOP_TEAL))
        .show(ui, |ui| {
            ui.add_space(90.0);
            ui.vertical_centered(|ui| {
                let card = egui::Frame::default()
                    .fill(theme::FACE_GRAY)
                    .inner_margin(egui::Margin::same(16))
                    .show(ui, |ui| {
                        ui.set_width(280.0);
                        ui.vertical_centered(|ui| {
                            ui.heading("RYMNET DESKTOP");
                            ui.add_space(8.0);
                        });
                        ui.separator();
                        ui.add_space(8.0);

                        ui.label("Username:");
                        ui.add_enabled(
                            !state.busy,
                            egui::TextEdit::singleline(&mut state.username).desired_width(f32::INFINITY),
                        );
                        ui.add_space(4.0);
                        ui.label("Password:");
                        ui.add_enabled(
                            !state.busy,
                            egui::TextEdit::singleline(&mut state.password)
                                .password(true)
                                .desired_width(f32::INFINITY),
                        );
                        ui.add_space(10.0);

                        let can_submit = !state.busy
                            && !state.username.trim().is_empty()
                            && !state.password.is_empty();
                        ui.vertical_centered(|ui| {
                            if ui
                                .add_enabled(can_submit, egui::Button::new(if state.busy { "Logging in..." } else { "Log In" }))
                                .clicked()
                            {
                                let (tx, rx) = mpsc::channel();
                                let username = state.username.trim().to_string();
                                let password = std::mem::take(&mut state.password);
                                let layout = layout.clone();
                                runtime.spawn(async move {
                                    let result = account::login(&layout, &username, &password).await;
                                    let _ = tx.send(result);
                                });
                                state.rx = Some(rx);
                                state.busy = true;
                                state.status = None;
                            }
                        });

                        if let Some(status) = &state.status {
                            ui.add_space(8.0);
                            let color = if state.status_is_error { theme::ERROR_MAROON } else { theme::TEXT_BLACK };
                            ui.colored_label(color, status);
                        }
                    });
                theme::bevel_box(ui, card.response.rect, true);
            });
        });

    None
}

fn show_desktop(
    ui: &mut egui::Ui,
    runtime: &tokio::runtime::Runtime,
    layout: &DataLayout,
    identity: &NodeIdentity,
    screen: &mut Screen,
) -> Option<Screen> {
    let Screen::Desktop(state) = screen else {
        return None;
    };

    if let Some(rx) = &state.rx {
        if let Ok(result) = rx.try_recv() {
            state.logging_out = false;
            state.rx = None;
            match result {
                Ok(summary) => {
                    return Some(Screen::Login(LoginState {
                        status: Some(format!(
                            "Logged out (synced {} file(s) to {}/{} peers, vfs version {}).",
                            summary.files_pushed, summary.peers_accepted, summary.peers_total, summary.vfs_version
                        )),
                        ..Default::default()
                    }));
                }
                Err(err) => {
                    state.status = Some(format!("Sync before logout failed: {err:#}"));
                    state.status_is_error = true;
                }
            }
        } else {
            ui.ctx().request_repaint();
        }
    }

    let title_bar = egui::Panel::top("title_bar")
        .frame(
            egui::Frame::default()
                .fill(theme::TITLE_NAVY)
                .inner_margin(egui::Margin::symmetric(8, 4)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.colored_label(
                    theme::HIGHLIGHT_WHITE,
                    format!("Rymnet Desktop - {}", state.account.username),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if state.logging_out { "Logging out..." } else { "Log Out" };
                    if ui.add_enabled(!state.logging_out, egui::Button::new(label)).clicked() {
                        let (tx, rx) = mpsc::channel();
                        let layout = layout.clone();
                        let username = state.account.username.clone();
                        let identity = state.account.identity.clone();
                        let cipher_key = state.account.cipher_key;
                        let cache_dir = state.account.cache_dir.clone();
                        let vfs_version = state.account.vfs_version;
                        runtime.spawn(async move {
                            let result =
                                account::push(&layout, &username, &identity, &cipher_key, &cache_dir, vfs_version)
                                    .await;
                            let _ = tx.send(result);
                        });
                        state.rx = Some(rx);
                        state.logging_out = true;
                        state.status = None;
                    }
                });
            });
        });
    theme::bevel_box(ui, title_bar.response.rect, true);

    match taskbar::show(ui, true) {
        Some(taskbar::StartAction::FileManager) => {
            state.apps.push(Box::new(filemanager::FileManagerApp::new()));
        }
        Some(taskbar::StartAction::Rymcode) => {
            state.apps.push(Box::new(rymcode::RymcodeApp::new()));
        }
        Some(taskbar::StartAction::Browser) => {
            state.apps.push(Box::new(browserapp::BrowserApp::new()));
        }
        Some(taskbar::StartAction::Terminal) => {
            state.apps.push(Box::new(terminal::TerminalApp::new()));
        }
        Some(taskbar::StartAction::Mail) => {
            state.apps.push(Box::new(mailapp::MailApp::new()));
        }
        Some(taskbar::StartAction::Irc) => {
            state.apps.push(Box::new(ircapp::IrcApp::new()));
        }
        Some(taskbar::StartAction::PdfReader) => {
            state.apps.push(Box::new(pdfreaderapp::PdfReaderApp::new()));
        }
        Some(taskbar::StartAction::Paint) => {
            state.apps.push(Box::new(paintapp::PaintApp::new()));
        }
        Some(taskbar::StartAction::Writer) => {
            state.apps.push(Box::new(writerapp::WriterApp::new()));
        }
        Some(taskbar::StartAction::Board) => {
            state.apps.push(Box::new(boardapp::BoardApp::new()));
        }
        None => {}
    }

    egui::CentralPanel::default()
        .frame(egui::Frame::default().fill(theme::DESKTOP_TEAL))
        .show(ui, |ui| {
            if let Some(status) = &state.status {
                let color = if state.status_is_error { theme::ERROR_MAROON } else { theme::HIGHLIGHT_WHITE };
                ui.colored_label(color, status);
            }
        });

    let ctx = ui.ctx().clone();
    let runtime_handle = runtime.handle().clone();
    let app_context = AppContext {
        vfs: &state.vfs,
        runtime: &runtime_handle,
        layout,
        identity,
        username: &state.account.username,
        mailbox_secret: &state.account.mailbox_secret,
    };
    let mut launched: Vec<Box<dyn RymApp>> = Vec::new();
    state.apps.retain_mut(|app| app.show(&ctx, &app_context, &mut launched));
    state.apps.extend(launched);

    // Files dropped on the desktop (from Finder, or from RAD around it) land in /Downloads.
    let (hovering, dropped) = ctx.input(|i| (!i.raw.hovered_files.is_empty(), i.raw.dropped_files.clone()));
    if hovering {
        drop_overlay(&ctx, "Drop to copy into /Downloads");
    }
    if !dropped.is_empty() {
        let (copied, errors) = import_dropped(&state.vfs, &dropped);
        state.status_is_error = !errors.is_empty();
        state.status = Some(if errors.is_empty() {
            format!("Copied {copied} item(s) into /Downloads. They sync to your peers when you log out.")
        } else {
            format!("Copied {copied} item(s) into /Downloads; failed: {}", errors.join("; "))
        });
        if copied > 0 {
            state.apps.push(Box::new(filemanager::FileManagerApp::at("/Downloads")));
        }
    }

    None
}

fn drop_overlay(ctx: &egui::Context, text: &str) {
    let screen = ctx.content_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("drop_overlay")));
    painter.rect_filled(screen, 0.0, egui::Color32::from_black_alpha(140));
    painter.text(screen.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(22.0), egui::Color32::WHITE);
}

/// Copies dropped files and folders into the account's `/Downloads`, renaming on clashes
/// (`name (2).ext`). Returns how many top-level items were copied and the failures.
fn import_dropped(vfs: &VirtualComputer, files: &[egui::DroppedFile]) -> (usize, Vec<String>) {
    let dest_dir = vfs.real_path("/Downloads");
    let mut copied = 0;
    let mut errors = Vec::new();
    if let Err(err) = std::fs::create_dir_all(&dest_dir) {
        return (0, vec![format!("/Downloads: {err}")]);
    }
    for f in files {
        let name = if f.name.is_empty() {
            f.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "dropped".into())
        } else {
            f.name.clone()
        };
        let dest = unique_path(&dest_dir, &name);
        let result = match (&f.path, &f.bytes) {
            (Some(src), _) => copy_recursive(src, &dest),
            (None, Some(bytes)) => std::fs::write(&dest, bytes).map_err(Into::into),
            _ => Err(anyhow::anyhow!("nothing to copy")),
        };
        match result {
            Ok(()) => copied += 1,
            Err(err) => errors.push(format!("{name}: {err:#}")),
        }
    }
    (copied, errors)
}

fn unique_path(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let p = std::path::Path::new(name);
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .expect("some free name")
}

fn copy_recursive(src: &std::path::Path, dest: &std::path::Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(src)?;
    if meta.is_dir() {
        std::fs::create_dir_all(dest)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &dest.join(entry.file_name()))?;
        }
    } else if meta.is_file() {
        std::fs::copy(src, dest)?;
    }
    // Symlinks are skipped: they would point outside the virtual computer.
    Ok(())
}

#[cfg(test)]
mod drop_tests {
    use super::*;

    #[test]
    fn dropped_files_and_folders_land_in_downloads_without_clobbering() {
        let base = std::env::temp_dir().join(format!("rymwin-drop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let src = base.join("src");
        std::fs::create_dir_all(src.join("folder/inner")).unwrap();
        std::fs::write(src.join("a.txt"), "one").unwrap();
        std::fs::write(src.join("folder/inner/b.txt"), "two").unwrap();
        let vfs = VirtualComputer::open(base.join("vfs")).unwrap();
        let drop = |p: std::path::PathBuf| egui::DroppedFile { name: p.file_name().unwrap().to_string_lossy().into(), path: Some(p), ..Default::default() };

        let (n, errs) = import_dropped(&vfs, &[drop(src.join("a.txt")), drop(src.join("folder"))]);
        assert_eq!((n, errs.len()), (2, 0));
        assert_eq!(vfs.read_to_string("/Downloads/a.txt").unwrap(), "one");
        assert_eq!(vfs.read_to_string("/Downloads/folder/inner/b.txt").unwrap(), "two");

        // Same name again: kept alongside, not overwritten.
        let (n, _) = import_dropped(&vfs, &[drop(src.join("a.txt"))]);
        assert_eq!(n, 1);
        assert!(vfs.real_path("/Downloads/a (2).txt").is_file());
        let _ = std::fs::remove_dir_all(&base);
    }
}
