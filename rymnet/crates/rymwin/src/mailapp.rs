use crate::app::{AppContext, RymApp};
use crate::mail;
use crate::theme;
use eframe::egui;
use std::collections::HashSet;
use std::sync::mpsc;

#[derive(PartialEq, Clone, Copy)]
enum Folder {
    Inbox,
    Sent,
}

/// A message this session composed and sent -- kept only in memory, for
/// this window's lifetime. There's no real Sent folder anywhere in
/// Rymnet (mail has no server-side "sent items" concept, see
/// BUILD_OVERVIEW.md), so this is honestly a local convenience, not
/// something that persists or syncs.
struct SentMessage {
    to: String,
    subject: String,
    body: String,
    sent_at: chrono::DateTime<chrono::Utc>,
    delivered: usize,
    total: usize,
}

struct ComposeState {
    to: String,
    cc: String,
    bcc: String,
    subject: String,
    body: String,
    in_reply_to: Option<String>,
    thread_id: Option<String>,
    sending: bool,
    status: Option<String>,
    rx: Option<mpsc::Receiver<Result<(usize, usize), String>>>,
}

/// The fifth desktop app: an Outlook-alike mail client over the same
/// sealed, signed mail every other front end in this project uses --
/// folder list on the left, message list top-center, reading pane
/// below it, and a separate compose window (real form fields, not a
/// sequential prompt -- this is a GUI, so unlike `rymsh`/`rymweb`'s
/// compose flow there's no reason not to show every field at once).
pub struct MailApp {
    folder: Folder,
    inbox: Vec<mail::InboxMessage>,
    sent: Vec<SentMessage>,
    selected: Option<usize>,
    read_ids: HashSet<String>,
    loading: bool,
    status: Option<String>,
    status_is_error: bool,
    inbox_rx: Option<mpsc::Receiver<Result<Vec<mail::InboxMessage>, String>>>,
    compose: Option<ComposeState>,
    save_input: String,
    save_status: Option<String>,
    save_rx: Option<mpsc::Receiver<Result<String, String>>>,
    first_frame: bool,
}

impl MailApp {
    pub fn new() -> Self {
        Self {
            folder: Folder::Inbox,
            inbox: Vec::new(),
            sent: Vec::new(),
            selected: None,
            read_ids: HashSet::new(),
            loading: false,
            status: None,
            status_is_error: false,
            inbox_rx: None,
            compose: None,
            save_input: String::new(),
            save_status: None,
            save_rx: None,
            first_frame: true,
        }
    }

    fn instance_id(&self) -> usize {
        self as *const Self as usize
    }

    fn refresh_inbox(&mut self, app: &AppContext) {
        self.loading = true;
        self.status = None;
        let (tx, rx) = mpsc::channel();
        let layout = app.layout.clone();
        let username = app.username.to_string();
        let identity = app.identity.clone();
        let mailbox_secret = app.mailbox_secret.clone();
        app.runtime.spawn(async move {
            let result = mail::inbox(&layout, &username, &identity, &mailbox_secret)
                .await
                .map_err(|err| format!("{err:#}"));
            let _ = tx.send(result);
        });
        self.inbox_rx = Some(rx);
    }

    fn poll_inbox(&mut self) {
        let Some(rx) = &self.inbox_rx else { return };
        let Ok(result) = rx.try_recv() else { return };
        self.inbox_rx = None;
        self.loading = false;
        match result {
            Ok(messages) => {
                let count = messages.len();
                self.inbox = messages;
                self.status = Some(format!("{count} message(s) in inbox."));
                self.status_is_error = false;
            }
            Err(err) => {
                self.status = Some(format!("error: {err}"));
                self.status_is_error = true;
            }
        }
    }

    /// Fetches+unseals the selected message's attachment and writes it
    /// into this account's vfs at `dest` -- see `mail::save_attachment`'s
    /// own doc comment for the fetch/verify/unseal pipeline.
    fn start_save_attachment(&mut self, app: &AppContext, dest: String) {
        let Some(msg) = self.selected.and_then(|i| self.inbox.get(i)) else { return };
        let Some(attachment) = msg.body.attachment.clone() else { return };
        self.save_status = Some("Saving...".to_string());
        let (tx, rx) = mpsc::channel();
        let layout = app.layout.clone();
        let identity = app.identity.clone();
        let mailbox_secret = app.mailbox_secret.clone();
        let real_dest = app.vfs.real_path(&dest);
        app.runtime.spawn(async move {
            let result = mail::save_attachment(&layout, &identity, &mailbox_secret, &attachment, &real_dest)
                .await
                .map(|()| dest)
                .map_err(|err| format!("{err:#}"));
            let _ = tx.send(result);
        });
        self.save_rx = Some(rx);
    }

    fn poll_save_attachment(&mut self) {
        let Some(rx) = &self.save_rx else { return };
        let Ok(result) = rx.try_recv() else { return };
        self.save_rx = None;
        self.save_status = Some(match result {
            Ok(dest) => format!("Saved to {dest}."),
            Err(err) => format!("error: {err}"),
        });
    }

    fn start_new(&mut self) {
        self.compose = Some(ComposeState {
            to: String::new(),
            cc: String::new(),
            bcc: String::new(),
            subject: String::new(),
            body: String::new(),
            in_reply_to: None,
            thread_id: None,
            sending: false,
            status: None,
            rx: None,
        });
    }

    fn start_reply(&mut self) {
        let Some(idx) = self.selected else { return };
        let Some(msg) = self.inbox.get(idx) else { return };
        self.compose = Some(ComposeState {
            to: msg.envelope.from.clone(),
            cc: String::new(),
            bcc: String::new(),
            subject: ensure_prefix(&msg.body.subject, "Re: "),
            body: String::new(),
            in_reply_to: Some(msg.envelope.id.clone()),
            thread_id: Some(msg.body.thread_id.clone().unwrap_or_else(|| msg.envelope.id.clone())),
            sending: false,
            status: None,
            rx: None,
        });
    }

    fn start_forward(&mut self) {
        let Some(idx) = self.selected else { return };
        let Some(msg) = self.inbox.get(idx) else { return };
        let quoted = format!(
            "\n\n---- Forwarded message ----\nFrom: {}\nSubject: {}\n\n{}",
            msg.envelope.from, msg.body.subject, msg.body.body
        );
        self.compose = Some(ComposeState {
            to: String::new(),
            cc: String::new(),
            bcc: String::new(),
            subject: ensure_prefix(&msg.body.subject, "Fwd: "),
            body: quoted,
            in_reply_to: None,
            thread_id: None,
            sending: false,
            status: None,
            rx: None,
        });
    }

    fn start_send(&mut self, app: &AppContext) {
        let Some(compose) = &mut self.compose else { return };
        compose.sending = true;
        compose.status = None;
        let (tx, rx) = mpsc::channel();
        let layout = app.layout.clone();
        let username = app.username.to_string();
        let identity = app.identity.clone();
        let to = compose.to.trim().to_string();
        let subject = compose.subject.clone();
        let cc = split_addresses(&compose.cc);
        let bcc = split_addresses(&compose.bcc);
        let body = compose.body.clone();
        let in_reply_to = compose.in_reply_to.clone();
        let thread_id = compose.thread_id.clone();
        app.runtime.spawn(async move {
            let result = mail::send(&layout, &username, &identity, &to, &subject, cc, bcc, &body, in_reply_to, thread_id, None)
                .await
                .map_err(|err| format!("{err:#}"));
            let _ = tx.send(result);
        });
        compose.rx = Some(rx);
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, app: &AppContext) {
        egui::Panel::top(egui::Id::new(("mail_toolbar", self.instance_id())))
            .frame(
                egui::Frame::default()
                    .fill(theme::FACE_GRAY)
                    .inner_margin(egui::Margin::symmetric(4, 4)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("\u{1f4dd} New Email").clicked() {
                        self.start_new();
                    }
                    let can_reply = self.folder == Folder::Inbox && self.selected.is_some();
                    if ui.add_enabled(can_reply, egui::Button::new("\u{21a9} Reply")).clicked() {
                        self.start_reply();
                    }
                    if ui.add_enabled(can_reply, egui::Button::new("\u{27a1} Forward")).clicked() {
                        self.start_forward();
                    }
                    if ui
                        .add_enabled(!self.loading, egui::Button::new(if self.loading { "Refreshing..." } else { "\u{27f3} Refresh" }))
                        .clicked()
                    {
                        self.refresh_inbox(app);
                    }
                });
                if let Some(status) = &self.status {
                    let color = if self.status_is_error { theme::ERROR_MAROON } else { theme::TEXT_BLACK };
                    ui.colored_label(color, status);
                }
            });
    }

    fn folder_list(&mut self, ui: &mut egui::Ui) {
        if ui
            .selectable_label(self.folder == Folder::Inbox, format!("\u{1f4e5} Inbox ({})", self.inbox.len()))
            .clicked()
        {
            self.folder = Folder::Inbox;
            self.selected = None;
        }
        if ui
            .selectable_label(self.folder == Folder::Sent, format!("\u{1f4e4} Sent ({})", self.sent.len()))
            .clicked()
        {
            self.folder = Folder::Sent;
            self.selected = None;
        }
        ui.add_space(8.0);
        ui.weak("Sent is this\nsession only --\nnot synced or\npersisted.");
    }

    fn message_list(&mut self, ui: &mut egui::Ui) {
        let mut clicked = None;
        let rows: Vec<(usize, String, bool)> = match self.folder {
            Folder::Inbox => self
                .inbox
                .iter()
                .enumerate()
                .map(|(i, m)| {
                    let unread = !self.read_ids.contains(&m.envelope.id);
                    let mark = if unread { "\u{25cf} " } else { "" };
                    let verified = if m.verified { "" } else { " [unverified]" };
                    (
                        i,
                        format!(
                            "{mark}{}  --  {}  ({}){verified}",
                            m.envelope.from,
                            m.body.subject,
                            m.envelope.sent_at.format("%Y-%m-%d %H:%M")
                        ),
                        unread,
                    )
                })
                .collect(),
            Folder::Sent => self
                .sent
                .iter()
                .enumerate()
                .map(|(i, m)| {
                    (
                        i,
                        format!(
                            "{}  --  {}  ({}, {}/{} peers)",
                            m.to,
                            m.subject,
                            m.sent_at.format("%Y-%m-%d %H:%M"),
                            m.delivered,
                            m.total
                        ),
                        false,
                    )
                })
                .collect(),
        };

        egui::ScrollArea::vertical()
            .id_salt(("mail_list_scroll", self.instance_id()))
            .show(ui, |ui| {
                if rows.is_empty() {
                    ui.weak(if self.loading { "Loading..." } else { "(empty)" });
                }
                for (i, label, _unread) in &rows {
                    let is_selected = self.selected == Some(*i);
                    if ui.selectable_label(is_selected, label).clicked() {
                        clicked = Some(*i);
                    }
                }
            });

        if let Some(i) = clicked {
            self.selected = Some(i);
            if self.folder == Folder::Inbox
                && let Some(msg) = self.inbox.get(i)
            {
                self.read_ids.insert(msg.envelope.id.clone());
            }
        }
    }

    fn reading_pane(&mut self, ui: &mut egui::Ui, app: &AppContext) {
        let mut save_clicked = None;
        egui::ScrollArea::vertical()
            .id_salt(("mail_reading_scroll", self.instance_id()))
            .show(ui, |ui| match self.folder {
                Folder::Inbox => {
                    let Some(msg) = self.selected.and_then(|i| self.inbox.get(i)) else {
                        ui.weak("Select a message to read it.");
                        return;
                    };
                    ui.label(format!("From:    {}", msg.envelope.from));
                    ui.label(format!("To:      {}", msg.envelope.to));
                    ui.label(format!("Subject: {}", msg.body.subject));
                    ui.label(format!("Date:    {}", msg.envelope.sent_at.format("%Y-%m-%d %H:%M")));
                    if msg.verified {
                        ui.colored_label(theme::TEXT_BLACK, "Verified: yes (signature matches sender's registered key)");
                    } else {
                        ui.colored_label(theme::ERROR_MAROON, "Verified: no -- sender identity could not be confirmed");
                    }
                    if let Some(attachment) = &msg.body.attachment {
                        ui.separator();
                        ui.label(format!("Attachment: {} ({} bytes, {})", attachment.name, attachment.size, attachment.mime));
                        ui.horizontal(|ui| {
                            ui.label("Save as:");
                            ui.add(egui::TextEdit::singleline(&mut self.save_input).desired_width(200.0));
                            if ui.button("Save").clicked() {
                                let dest = if self.save_input.trim().is_empty() {
                                    attachment.name.clone()
                                } else {
                                    self.save_input.trim().to_string()
                                };
                                save_clicked = Some(dest);
                            }
                        });
                        if let Some(status) = &self.save_status {
                            ui.weak(status);
                        }
                    }
                    ui.separator();
                    ui.label(&msg.body.body);
                }
                Folder::Sent => {
                    let Some(msg) = self.selected.and_then(|i| self.sent.get(i)) else {
                        ui.weak("Select a message to read it.");
                        return;
                    };
                    ui.label(format!("To:      {}", msg.to));
                    ui.label(format!("Subject: {}", msg.subject));
                    ui.label(format!(
                        "Sent:    {} (delivered to {}/{} peers)",
                        msg.sent_at.format("%Y-%m-%d %H:%M"),
                        msg.delivered,
                        msg.total
                    ));
                    ui.separator();
                    ui.label(&msg.body);
                }
            });
        if let Some(dest) = save_clicked {
            self.start_save_attachment(app, dest);
        }
    }

    fn compose_window(&mut self, ctx: &egui::Context, app: &AppContext) {
        if let Some(compose) = &mut self.compose
            && let Some(rx) = &compose.rx
            && let Ok(result) = rx.try_recv()
        {
            compose.rx = None;
            compose.sending = false;
            match result {
                Ok((delivered, total)) => {
                    self.sent.push(SentMessage {
                        to: compose.to.clone(),
                        subject: compose.subject.clone(),
                        body: compose.body.clone(),
                        sent_at: chrono::Utc::now(),
                        delivered,
                        total,
                    });
                    self.compose = None;
                    self.status = Some(format!("Message sent (delivered to {delivered}/{total} peers)."));
                    self.status_is_error = false;
                }
                Err(err) => compose.status = Some(err),
            }
        }

        let Some(_) = &self.compose else { return };
        let instance_id = self.instance_id();
        let mut open = true;
        let mut send_clicked = false;
        {
            let compose = self.compose.as_mut().expect("checked above");
            egui::Window::new("New Message")
                .id(egui::Id::new(("mail_compose", instance_id)))
                .default_size([420.0, 380.0])
                .open(&mut open)
                .show(ctx, |ui| {
                    egui::Grid::new(("mail_compose_grid", instance_id))
                        .num_columns(2)
                        .show(ui, |ui| {
                            ui.label("To:");
                            ui.add(egui::TextEdit::singleline(&mut compose.to).desired_width(f32::INFINITY));
                            ui.end_row();
                            ui.label("Cc:");
                            ui.add(egui::TextEdit::singleline(&mut compose.cc).desired_width(f32::INFINITY));
                            ui.end_row();
                            ui.label("Bcc:");
                            ui.add(egui::TextEdit::singleline(&mut compose.bcc).desired_width(f32::INFINITY));
                            ui.end_row();
                            ui.label("Subject:");
                            ui.add(egui::TextEdit::singleline(&mut compose.subject).desired_width(f32::INFINITY));
                            ui.end_row();
                        });
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .id_salt(("mail_compose_body_scroll", instance_id))
                        .max_height(200.0)
                        .show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::multiline(&mut compose.body)
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(10),
                            );
                        });
                    ui.separator();
                    ui.horizontal(|ui| {
                        let can_send = !compose.sending && !compose.to.trim().is_empty();
                        let label = if compose.sending { "Sending..." } else { "Send" };
                        if ui.add_enabled(can_send, egui::Button::new(label)).clicked() {
                            send_clicked = true;
                        }
                        if let Some(status) = &compose.status {
                            ui.colored_label(theme::ERROR_MAROON, status);
                        }
                    });
                });
        }
        if send_clicked {
            self.start_send(app);
        }
        if !open {
            self.compose = None;
        }
    }
}

impl RymApp for MailApp {
    fn title(&self) -> String {
        "Mail".to_string()
    }

    fn show(&mut self, ctx: &egui::Context, app: &AppContext, _launch: &mut Vec<Box<dyn RymApp>>) -> bool {
        if self.first_frame {
            self.first_frame = false;
            self.refresh_inbox(app);
        }
        self.poll_inbox();
        self.poll_save_attachment();
        if self.inbox_rx.is_some() || self.compose.as_ref().is_some_and(|c| c.rx.is_some()) || self.save_rx.is_some() {
            ctx.request_repaint();
        }

        let mut open = true;
        theme::DecoratedWindow::new(egui::Id::new(("mail_window", self.instance_id())), self.title())
            .icon("\u{2709}")
            .default_size([680.0, 460.0])
            .show(ctx, &mut open, |ui| {
                self.toolbar(ui, app);
                egui::Panel::left(egui::Id::new(("mail_folder_panel", self.instance_id())))
                    .default_size(110.0)
                    .resizable(false)
                    .show(ui, |ui| {
                        self.folder_list(ui);
                    });
                egui::Panel::top(egui::Id::new(("mail_list_panel", self.instance_id())))
                    .default_size(160.0)
                    .show(ui, |ui| {
                        self.message_list(ui);
                    });
                self.reading_pane(ui, app);
            });

        self.compose_window(ctx, app);
        open
    }
}

fn ensure_prefix(subject: &str, prefix: &str) -> String {
    if subject.to_ascii_lowercase().starts_with(&prefix.to_ascii_lowercase()) {
        subject.to_string()
    } else {
        format!("{prefix}{subject}")
    }
}

fn split_addresses(input: &str) -> Vec<String> {
    input.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect()
}
