use crate::app::{AppContext, RymApp};
use crate::irc;
use crate::mail;
use crate::theme;
use eframe::egui;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How often the active channel is re-fetched while this window is
/// open. Mail's own refresh is manual-button-only; chat needs an actual
/// timer to feel anything like "instant messaging" given there's no
/// push -- see `irc.rs`'s module doc for why polling is the whole
/// design, not a shortcut.
const POLL_INTERVAL: Duration = Duration::from_secs(4);

type FetchResult = (String, Result<Vec<irc::ChannelMessage>, String>);

struct DmState {
    to: String,
    input: String,
    sending: bool,
    status: Option<String>,
    rx: Option<mpsc::Receiver<Result<(usize, usize), String>>>,
}

/// The sixth desktop app: channels on the left (like Mail's folder
/// list), scrollback + input box for the active one. Channel messages
/// are signed but not encrypted (see `rymnet-chat`'s doc comments for
/// why), so -- unlike Mail -- there's no secret needed to read them;
/// `AppContext.mailbox_secret` is only touched for the DM panel, which
/// is just Mail's own `send` under a chat-styled input box.
pub struct IrcApp {
    channels: Vec<String>,
    active: usize,
    join_input: String,
    messages: std::collections::HashMap<String, Vec<irc::ChannelMessage>>,
    compose_input: String,
    send_rx: Option<mpsc::Receiver<Result<(usize, usize), String>>>,
    fetch_rx: Option<mpsc::Receiver<FetchResult>>,
    dm: Option<DmState>,
    status: Option<String>,
    status_is_error: bool,
    last_poll: Instant,
    first_frame: bool,
}

impl IrcApp {
    pub fn new() -> Self {
        Self {
            channels: Vec::new(),
            active: 0,
            join_input: String::new(),
            messages: std::collections::HashMap::new(),
            compose_input: String::new(),
            send_rx: None,
            fetch_rx: None,
            dm: None,
            status: None,
            status_is_error: false,
            last_poll: Instant::now() - POLL_INTERVAL,
            first_frame: true,
        }
    }

    fn instance_id(&self) -> usize {
        self as *const Self as usize
    }

    fn active_channel(&self) -> Option<String> {
        self.channels.get(self.active).cloned()
    }

    fn join_channel(&mut self, app: &AppContext, channel: String) {
        if let Err(err) = rymnet_chat::validate_channel(&channel) {
            self.status = Some(format!("{err:#}"));
            self.status_is_error = true;
            return;
        }
        if let Some(idx) = self.channels.iter().position(|c| c == &channel) {
            self.active = idx;
        } else {
            self.channels.push(channel.clone());
            self.active = self.channels.len() - 1;
        }
        self.last_poll = Instant::now() - POLL_INTERVAL;
        self.refresh_active(app);
    }

    fn refresh_active(&mut self, app: &AppContext) {
        let Some(channel) = self.active_channel() else { return };
        if self.fetch_rx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let layout = app.layout.clone();
        let identity = app.identity.clone();
        let channel_for_task = channel.clone();
        app.runtime.spawn(async move {
            let result = irc::fetch(&layout, &identity, &channel_for_task)
                .await
                .map_err(|err| format!("{err:#}"));
            let _ = tx.send((channel_for_task, result));
        });
        self.fetch_rx = Some(rx);
    }

    fn poll_fetch(&mut self) {
        let Some(rx) = &self.fetch_rx else { return };
        let Ok((channel, result)) = rx.try_recv() else { return };
        self.fetch_rx = None;
        match result {
            Ok(messages) => {
                self.messages.insert(channel, messages);
            }
            Err(err) => {
                self.status = Some(format!("error: {err}"));
                self.status_is_error = true;
            }
        }
    }

    fn send_active(&mut self, app: &AppContext) {
        let Some(channel) = self.active_channel() else { return };
        let text = self.compose_input.trim().to_string();
        if text.is_empty() || self.send_rx.is_some() {
            return;
        }
        self.compose_input.clear();
        let (tx, rx) = mpsc::channel();
        let layout = app.layout.clone();
        let username = app.username.to_string();
        let identity = app.identity.clone();
        app.runtime.spawn(async move {
            let result = irc::send(&layout, &username, &identity, &channel, &text)
                .await
                .map_err(|err| format!("{err:#}"));
            let _ = tx.send(result);
        });
        self.send_rx = Some(rx);
    }

    fn poll_send(&mut self, app: &AppContext) {
        let Some(rx) = &self.send_rx else { return };
        let Ok(result) = rx.try_recv() else { return };
        self.send_rx = None;
        match result {
            Ok((delivered, total)) => {
                self.status = Some(format!("sent (delivered to {delivered}/{total} peer(s))"));
                self.status_is_error = false;
                // Pull the just-sent message back in right away rather
                // than waiting for the next poll tick.
                self.refresh_active(app);
            }
            Err(err) => {
                self.status = Some(format!("send failed: {err}"));
                self.status_is_error = true;
            }
        }
    }

    fn start_dm(&mut self, to: String) {
        self.dm = Some(DmState {
            to,
            input: String::new(),
            sending: false,
            status: None,
            rx: None,
        });
    }

    fn send_dm(&mut self, app: &AppContext) {
        let Some(dm) = &mut self.dm else { return };
        let to = dm.to.trim().to_string();
        let text = dm.input.trim().to_string();
        if to.is_empty() || text.is_empty() {
            return;
        }
        dm.sending = true;
        dm.status = None;
        let (tx, rx) = mpsc::channel();
        let layout = app.layout.clone();
        let username = app.username.to_string();
        let identity = app.identity.clone();
        app.runtime.spawn(async move {
            let result = mail::send(&layout, &username, &identity, &to, "(chat)", Vec::new(), Vec::new(), &text, None, None, None)
                .await
                .map_err(|err| format!("{err:#}"));
            let _ = tx.send(result);
        });
        dm.rx = Some(rx);
    }

    fn poll_dm(&mut self) {
        let Some(dm) = &mut self.dm else { return };
        let Some(rx) = &dm.rx else { return };
        let Ok(result) = rx.try_recv() else { return };
        dm.rx = None;
        dm.sending = false;
        match result {
            Ok((delivered, total)) => {
                dm.status = Some(format!("delivered to {delivered}/{total} peer(s)"));
                dm.input.clear();
            }
            Err(err) => dm.status = Some(err),
        }
    }

    fn channel_list(&mut self, ui: &mut egui::Ui, app: &AppContext) {
        let mut switch_to = None;
        for (i, channel) in self.channels.iter().enumerate() {
            if ui.selectable_label(self.active == i, channel).clicked() {
                switch_to = Some(i);
            }
        }
        if let Some(i) = switch_to {
            self.active = i;
            self.last_poll = Instant::now() - POLL_INTERVAL;
        }
        ui.separator();
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.join_input).hint_text("#channel").desired_width(90.0));
            if ui.button("Join").clicked() && !self.join_input.trim().is_empty() {
                let channel = self.join_input.trim().to_string();
                self.join_input.clear();
                self.join_channel(app, channel);
            }
        });
        ui.add_space(8.0);
        if ui.button("Direct Message...").clicked() {
            self.start_dm(String::new());
        }
    }

    fn scrollback(&mut self, ui: &mut egui::Ui) {
        let Some(channel) = self.active_channel() else {
            ui.weak("Join a channel to start chatting.");
            return;
        };
        egui::ScrollArea::vertical()
            .id_salt(("irc_scrollback", self.instance_id()))
            .stick_to_bottom(true)
            .show(ui, |ui| {
                let empty = Vec::new();
                let messages = self.messages.get(&channel).unwrap_or(&empty);
                if messages.is_empty() {
                    ui.weak("(no messages yet)");
                }
                for message in messages {
                    let mark = if message.verified { "" } else { " [unverified]" };
                    ui.label(format!(
                        "[{}] {}{}: {}",
                        message.message.sent_at.format("%H:%M"),
                        message.message.from,
                        mark,
                        message.message.body
                    ));
                }
            });
    }

    fn compose_row(&mut self, ui: &mut egui::Ui, app: &AppContext) {
        ui.horizontal(|ui| {
            let enabled = self.active_channel().is_some() && self.send_rx.is_none();
            let response = ui.add_enabled(
                enabled,
                egui::TextEdit::singleline(&mut self.compose_input).desired_width(f32::INFINITY),
            );
            let enter_pressed = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let send_clicked = ui.add_enabled(enabled, egui::Button::new("Send")).clicked();
            if enabled && (enter_pressed || send_clicked) {
                self.send_active(app);
            }
        });
        if let Some(status) = &self.status {
            let color = if self.status_is_error { theme::ERROR_MAROON } else { theme::TEXT_BLACK };
            ui.colored_label(color, status);
        }
    }

    fn dm_window(&mut self, ctx: &egui::Context, app: &AppContext) {
        self.poll_dm();
        let Some(_) = &self.dm else { return };
        let instance_id = self.instance_id();
        let mut open = true;
        let mut send_clicked = false;
        {
            let dm = self.dm.as_mut().expect("checked above");
            egui::Window::new("Direct Message")
                .id(egui::Id::new(("irc_dm", instance_id)))
                .default_size([340.0, 160.0])
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("To:");
                        ui.add(egui::TextEdit::singleline(&mut dm.to).desired_width(f32::INFINITY));
                    });
                    ui.add(egui::TextEdit::multiline(&mut dm.input).desired_rows(4).desired_width(f32::INFINITY));
                    ui.horizontal(|ui| {
                        let can_send = !dm.sending && !dm.to.trim().is_empty() && !dm.input.trim().is_empty();
                        let label = if dm.sending { "Sending..." } else { "Send" };
                        if ui.add_enabled(can_send, egui::Button::new(label)).clicked() {
                            send_clicked = true;
                        }
                        if let Some(status) = &dm.status {
                            ui.weak(status);
                        }
                    });
                    ui.weak("Direct messages are sealed mail, not a channel broadcast -- only the recipient can read them.");
                });
        }
        if send_clicked {
            self.send_dm(app);
        }
        if !open {
            self.dm = None;
        }
    }
}

impl RymApp for IrcApp {
    fn title(&self) -> String {
        "Chat".to_string()
    }

    fn show(&mut self, ctx: &egui::Context, app: &AppContext, _launch: &mut Vec<Box<dyn RymApp>>) -> bool {
        if self.first_frame {
            self.first_frame = false;
            self.join_channel(app, "#general".to_string());
        }
        self.poll_fetch();
        self.poll_send(app);

        if self.fetch_rx.is_none() && self.active_channel().is_some() && self.last_poll.elapsed() >= POLL_INTERVAL {
            self.refresh_active(app);
            self.last_poll = Instant::now();
        }
        // Mail's refresh is manual-button-only and never needs this; chat
        // needs a repaint scheduled even with zero user input, so the
        // elapsed() check above actually gets re-evaluated on a timer.
        ctx.request_repaint_after(Duration::from_millis(500));

        let mut open = true;
        theme::DecoratedWindow::new(egui::Id::new(("irc_window", self.instance_id())), self.title())
            .icon("\u{1f4ac}")
            .default_size([560.0, 420.0])
            .show(ctx, &mut open, |ui| {
                egui::Panel::left(egui::Id::new(("irc_channel_panel", self.instance_id())))
                    .default_size(140.0)
                    .resizable(false)
                    .show(ui, |ui| {
                        self.channel_list(ui, app);
                    });
                egui::Panel::bottom(egui::Id::new(("irc_compose_panel", self.instance_id())))
                    .default_size(50.0)
                    .resizable(false)
                    .show(ui, |ui| {
                        self.compose_row(ui, app);
                    });
                self.scrollback(ui);
            });

        self.dm_window(ctx, app);
        open
    }
}
