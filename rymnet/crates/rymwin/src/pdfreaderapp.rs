use crate::app::{AppContext, RymApp};
use crate::pdf::{self, Command, Event, PdfSession};
use crate::theme;
use eframe::egui;

const MIN_ZOOM_WIDTH: i32 = 300;
const MAX_ZOOM_WIDTH: i32 = 3000;
const ZOOM_STEP: i32 = 200;

/// The seventh desktop app: a real, visually-rendered PDF viewer, using
/// `pdfium-render` (Chrome's own PDF engine) instead of any pure-Rust
/// alternative -- see `pdf.rs`'s own doc comment for why that's the only
/// native (non-Rust) runtime dependency anywhere in this workspace, and
/// why loading/rendering happens on its own dedicated thread rather than
/// this app's `AppContext.runtime`.
pub struct PdfReaderApp {
    session: Option<PdfSession>,
    open_path: Option<String>,
    page_count: u16,
    current_page: u16,
    zoom_width: i32,
    texture: Option<egui::TextureHandle>,
    textured_page: Option<u16>,
    needs_password: bool,
    password_input: String,
    status: Option<String>,
    error: Option<String>,
    show_tree: bool,
}

impl PdfReaderApp {
    pub fn new() -> Self {
        Self {
            session: None,
            open_path: None,
            page_count: 0,
            current_page: 0,
            zoom_width: 900,
            texture: None,
            textured_page: None,
            needs_password: false,
            password_input: String::new(),
            status: None,
            error: None,
            show_tree: true,
        }
    }

    fn instance_id(&self) -> usize {
        self as *const Self as usize
    }

    fn open(&mut self, app: &AppContext, path: String) {
        if self.session.is_none() {
            self.session = Some(pdf::spawn_pdf_session());
        }
        self.open_path = Some(path.clone());
        self.page_count = 0;
        self.current_page = 0;
        self.texture = None;
        self.textured_page = None;
        self.needs_password = false;
        self.password_input.clear();
        self.error = None;
        self.status = Some(format!("Opening {path}..."));
        self.send_open(app, None);
    }

    fn send_open(&self, app: &AppContext, password: Option<String>) {
        let (Some(session), Some(path)) = (&self.session, &self.open_path) else { return };
        let real_path = app.vfs.real_path(path);
        let _ = session.command_tx.send(Command::Open { path: real_path, password });
    }

    fn submit_password(&mut self, app: &AppContext) {
        if self.password_input.is_empty() {
            return;
        }
        self.status = Some("Opening...".to_string());
        self.send_open(app, Some(self.password_input.clone()));
    }

    fn request_render(&self, page: u16) {
        let Some(session) = &self.session else { return };
        let _ = session.command_tx.send(Command::RenderPage { page_index: page, target_width: self.zoom_width });
    }

    fn go_to_page(&mut self, page: u16) {
        if self.page_count == 0 || page >= self.page_count {
            return;
        }
        self.current_page = page;
        self.request_render(page);
    }

    fn set_zoom(&mut self, width: i32) {
        self.zoom_width = width.clamp(MIN_ZOOM_WIDTH, MAX_ZOOM_WIDTH);
        if self.page_count > 0 {
            self.request_render(self.current_page);
        }
    }

    /// Drains every event the worker thread has sent since the last
    /// frame. Non-blocking (`try_recv`), the same pattern
    /// `terminal.rs`'s `poll_run_session`/`poll_browse` already use for
    /// their own background workers.
    fn poll(&mut self, ctx: &egui::Context) {
        let Some(session) = &self.session else { return };
        loop {
            match session.events_rx.try_recv() {
                Ok(Event::Opened { page_count }) => {
                    self.page_count = page_count;
                    self.current_page = 0;
                    self.needs_password = false;
                    self.status = None;
                    self.request_render(0);
                }
                Ok(Event::PasswordRequired) => {
                    self.needs_password = true;
                    self.status = None;
                }
                Ok(Event::PageRendered { page_index, width, height, rgba }) => {
                    // A stray render for a page the user has since
                    // navigated away from -- discard it rather than
                    // showing a frame that doesn't match `current_page`.
                    if page_index != self.current_page {
                        continue;
                    }
                    let image = egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &rgba);
                    self.texture = Some(ctx.load_texture(
                        format!("pdf_page_{}", self.instance_id()),
                        image,
                        egui::TextureOptions::LINEAR,
                    ));
                    self.textured_page = Some(page_index);
                    self.status = None;
                }
                Ok(Event::Error(message)) => {
                    self.error = Some(message);
                    self.status = None;
                }
                Err(_) => break,
            }
        }
    }

    fn tree_pane(&mut self, ui: &mut egui::Ui, app: &AppContext) {
        egui::ScrollArea::vertical()
            .id_salt(("pdf_tree", self.instance_id()))
            .show(ui, |ui| {
                self.tree_node(ui, app, "/".to_string(), 0);
            });
    }

    fn tree_node(&mut self, ui: &mut egui::Ui, app: &AppContext, path: String, depth: usize) {
        if depth > 8 {
            return;
        }
        let Ok(entries) = app.vfs.list_entries(&path) else { return };
        for entry in entries {
            let child_path = if path.ends_with('/') { format!("{path}{}", entry.name) } else { format!("{path}/{}", entry.name) };
            ui.horizontal(|ui| {
                ui.add_space(depth as f32 * 12.0);
                if entry.is_dir {
                    ui.label(format!("\u{1f4c1} {}", entry.name));
                } else {
                    let is_open = self.open_path.as_deref() == Some(child_path.as_str());
                    let icon = if entry.name.to_ascii_lowercase().ends_with(".pdf") { "\u{1f4d5}" } else { "\u{1f4c4}" };
                    if ui.selectable_label(is_open, format!("{icon} {}", entry.name)).clicked() {
                        self.open(app, child_path.clone());
                    }
                }
            });
            if entry.is_dir {
                self.tree_node(ui, app, child_path, depth + 1);
            }
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button(if self.show_tree { "\u{25c0} Files" } else { "\u{25b6} Files" }).clicked() {
                self.show_tree = !self.show_tree;
            }
            ui.separator();
            let has_pages = self.page_count > 0;
            if ui.add_enabled(has_pages && self.current_page > 0, egui::Button::new("\u{25c0} Prev")).clicked() {
                self.go_to_page(self.current_page.saturating_sub(1));
            }
            if has_pages {
                ui.label(format!("Page {} / {}", self.current_page + 1, self.page_count));
            } else {
                ui.weak("No PDF open");
            }
            if ui.add_enabled(has_pages && self.current_page + 1 < self.page_count, egui::Button::new("Next \u{25b6}")).clicked() {
                self.go_to_page(self.current_page + 1);
            }
            ui.separator();
            if ui.add_enabled(has_pages, egui::Button::new("Zoom \u{2212}")).clicked() {
                self.set_zoom(self.zoom_width - ZOOM_STEP);
            }
            if ui.add_enabled(has_pages, egui::Button::new("Zoom +")).clicked() {
                self.set_zoom(self.zoom_width + ZOOM_STEP);
            }
        });
    }

    fn content(&mut self, ui: &mut egui::Ui, app: &AppContext) {
        if self.needs_password {
            ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                ui.label("This PDF is password-protected.");
                ui.horizontal(|ui| {
                    ui.label("Password:");
                    let response = ui.add(egui::TextEdit::singleline(&mut self.password_input).password(true));
                    let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if ui.button("Open").clicked() || enter {
                        self.submit_password(app);
                    }
                });
            });
            return;
        }
        if let Some(error) = &self.error {
            ui.colored_label(theme::ERROR_MAROON, error);
            return;
        }
        if let Some(status) = &self.status {
            ui.weak(status);
            return;
        }
        let Some(texture) = &self.texture else {
            ui.weak("Open a PDF from the file list on the left.");
            return;
        };
        egui::ScrollArea::both().id_salt(("pdf_page_scroll", self.instance_id())).show(ui, |ui| {
            ui.image((texture.id(), texture.size_vec2()));
        });
    }
}

impl RymApp for PdfReaderApp {
    fn title(&self) -> String {
        "PDF Reader".to_string()
    }

    fn show(&mut self, ctx: &egui::Context, app: &AppContext, _launch: &mut Vec<Box<dyn RymApp>>) -> bool {
        self.poll(ctx);
        // A render/open request may still be in flight on the worker
        // thread; keep polling until it lands instead of waiting for the
        // next unrelated repaint.
        if self.session.is_some() && (self.status.is_some() || self.textured_page != Some(self.current_page)) {
            ctx.request_repaint();
        }

        let mut open = true;
        theme::DecoratedWindow::new(egui::Id::new(("pdf_window", self.instance_id())), self.title())
            .icon("\u{1f4d5}")
            .default_size([700.0, 520.0])
            .show(ctx, &mut open, |ui| {
                self.toolbar(ui);
                ui.separator();
                if self.show_tree {
                    egui::Panel::left(egui::Id::new(("pdf_tree_panel", self.instance_id())))
                        .default_size(180.0)
                        .show(ui, |ui| {
                            self.tree_pane(ui, app);
                        });
                }
                self.content(ui, app);
            });
        open
    }
}

