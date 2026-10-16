use crate::app::{AppContext, RymApp};
use crate::browse::{self, FetchOutcome};
use crate::theme;
use eframe::egui;
use rymnet_storage::PeerConfig;
use std::sync::mpsc;

enum PageContent {
    Site(browse::Page),
    SearchResults { query: String, hits: Vec<browse::SearchHit> },
    Error(String),
}

/// A small clone of the early-IE chrome: Back/Forward/Refresh/Home
/// toolbar, an address bar, a content pane, and a status bar -- over the
/// same peer-protocol page fetch every other front end in this project
/// uses. There's nothing to execute on a rymnet page (no JS, no forms,
/// no iframes), so "rendering" here means the same thing it means in
/// `rymsh`'s lynx clone: extract the title, the body text, and the
/// links, and lay them out.
pub struct BrowserApp {
    address_bar: String,
    history: Vec<String>,
    history_index: usize,
    content: Option<PageContent>,
    status: String,
    status_is_error: bool,
    loading: bool,
    rx: Option<mpsc::Receiver<Result<FetchOutcome, String>>>,
}

impl BrowserApp {
    pub fn new() -> Self {
        Self {
            address_bar: String::new(),
            history: Vec::new(),
            history_index: 0,
            content: None,
            status: "Ready".to_string(),
            status_is_error: false,
            loading: false,
            rx: None,
        }
    }

    fn instance_id(&self) -> usize {
        self as *const Self as usize
    }

    fn submit(&mut self, app: &AppContext) {
        let input = self.address_bar.trim().to_string();
        if input.is_empty() {
            return;
        }
        let path = if input.contains(".rym") {
            input
        } else {
            format!("{}/?q={}", browse::SEARCH_DOMAIN, browse::percent_encode(&input))
        };
        self.navigate(app, path);
    }

    fn navigate(&mut self, app: &AppContext, path: String) {
        self.history.truncate(self.history_index + 1);
        if self.history.last() != Some(&path) {
            if !self.history.is_empty() {
                self.history_index += 1;
            }
            self.history.truncate(self.history_index);
            self.history.push(path.clone());
        }
        self.load(app, path);
    }

    fn go_back(&mut self, app: &AppContext) {
        if self.history_index > 0 {
            self.history_index -= 1;
            let path = self.history[self.history_index].clone();
            self.load(app, path);
        }
    }

    fn go_forward(&mut self, app: &AppContext) {
        if self.history_index + 1 < self.history.len() {
            self.history_index += 1;
            let path = self.history[self.history_index].clone();
            self.load(app, path);
        }
    }

    fn refresh(&mut self, app: &AppContext) {
        if let Some(path) = self.history.get(self.history_index).cloned() {
            self.load(app, path);
        }
    }

    fn go_home(&mut self) {
        self.content = None;
        self.address_bar.clear();
        self.status = "Ready".to_string();
        self.status_is_error = false;
        self.loading = false;
        self.rx = None;
    }

    fn load(&mut self, app: &AppContext, path: String) {
        self.address_bar = path.clone();
        self.loading = true;
        self.status = format!("Connecting to {path}...");
        self.status_is_error = false;
        let peers = match PeerConfig::load(app.layout.peers_path()) {
            Ok(peers) => peers,
            Err(err) => {
                self.status = format!("Error: {err:#}");
                self.status_is_error = true;
                self.content = Some(PageContent::Error(format!("{err:#}")));
                self.loading = false;
                return;
            }
        };
        let (tx, rx) = mpsc::channel();
        let identity = app.identity.clone();
        app.runtime.spawn(async move {
            let result = browse::fetch_for_path(&identity, &peers, &path).await;
            let _ = tx.send(result);
        });
        self.rx = Some(rx);
    }
}

impl RymApp for BrowserApp {
    fn title(&self) -> String {
        "Rymnet Browser".to_string()
    }

    fn show(&mut self, ctx: &egui::Context, app: &AppContext, _launch: &mut Vec<Box<dyn RymApp>>) -> bool {
        if let Some(rx) = &self.rx {
            if let Ok(result) = rx.try_recv() {
                self.loading = false;
                self.rx = None;
                match result {
                    Ok(FetchOutcome::Page(page)) => {
                        self.status = "Done".to_string();
                        self.status_is_error = false;
                        self.content = Some(PageContent::Site(page));
                    }
                    Ok(FetchOutcome::Search { query, hits }) => {
                        self.status = "Done".to_string();
                        self.status_is_error = false;
                        self.content = Some(PageContent::SearchResults { query, hits });
                    }
                    Err(err) => {
                        self.status = format!("Error: {err}");
                        self.status_is_error = true;
                        self.content = Some(PageContent::Error(err));
                    }
                }
            } else {
                ctx.request_repaint();
            }
        }

        let mut open = true;
        theme::DecoratedWindow::new(egui::Id::new(("browser_window", self.instance_id())), self.title())
            .icon("\u{1f310}")
            .default_size([560.0, 420.0])
            .show(ctx, &mut open, |ui| {
                self.toolbar(ui, app);
                self.status_bar(ui);
                self.content_area(ui, app);
            });
        open
    }
}

impl BrowserApp {
    fn toolbar(&mut self, ui: &mut egui::Ui, app: &AppContext) {
        let mut action: Option<ToolbarAction> = None;
        egui::Panel::top(egui::Id::new(("browser_toolbar", self.instance_id())))
            .frame(
                egui::Frame::default()
                    .fill(theme::FACE_GRAY)
                    .inner_margin(egui::Margin::symmetric(4, 4)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui.add_enabled(self.history_index > 0, egui::Button::new("\u{25c0} Back")).clicked() {
                        action = Some(ToolbarAction::Back);
                    }
                    if ui
                        .add_enabled(self.history_index + 1 < self.history.len(), egui::Button::new("\u{25b6} Forward"))
                        .clicked()
                    {
                        action = Some(ToolbarAction::Forward);
                    }
                    if ui.add_enabled(!self.history.is_empty(), egui::Button::new("\u{27f3} Refresh")).clicked() {
                        action = Some(ToolbarAction::Refresh);
                    }
                    if ui.button("\u{1f3e0} Home").clicked() {
                        action = Some(ToolbarAction::Home);
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Address:");
                    // The "Go" button must claim its space first -- an
                    // address bar with `desired_width(INFINITY)` placed
                    // before it in a left-to-right layout eats all
                    // remaining space and pushes Go off the visible
                    // window entirely. Laying out right-to-left and
                    // adding Go first pins it to the right edge, then
                    // the text edit fills exactly what's left.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let go_clicked = ui.button("Go").clicked();
                        let response =
                            ui.add(egui::TextEdit::singleline(&mut self.address_bar).desired_width(ui.available_width()));
                        let enter_pressed = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if go_clicked || enter_pressed {
                            action = Some(ToolbarAction::Go);
                        }
                    });
                });
            });

        match action {
            Some(ToolbarAction::Back) => self.go_back(app),
            Some(ToolbarAction::Forward) => self.go_forward(app),
            Some(ToolbarAction::Refresh) => self.refresh(app),
            Some(ToolbarAction::Home) => self.go_home(),
            Some(ToolbarAction::Go) => self.submit(app),
            None => {}
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom(egui::Id::new(("browser_status", self.instance_id())))
            .frame(
                egui::Frame::default()
                    .fill(theme::FACE_GRAY)
                    .inner_margin(egui::Margin::symmetric(6, 2)),
            )
            .exact_size(22.0)
            .show(ui, |ui| {
                let icon = if self.loading {
                    "\u{231b}"
                } else if self.status_is_error {
                    "\u{274c}"
                } else {
                    "\u{2705}"
                };
                let color = if self.status_is_error { theme::ERROR_MAROON } else { theme::TEXT_BLACK };
                ui.colored_label(color, format!("{icon} {}", self.status));
            });
    }

    fn content_area(&mut self, ui: &mut egui::Ui, app: &AppContext) {
        let mut clicked_link: Option<String> = None;
        egui::ScrollArea::vertical()
            .id_salt(("browser_content", self.instance_id()))
            .show(ui, |ui| match &self.content {
                None => {
                    ui.weak("Type a domain (e.g. example.rym) or a search phrase into the address bar and press Go.");
                }
                Some(PageContent::Error(err)) => {
                    ui.heading("This page can't be displayed");
                    ui.separator();
                    ui.colored_label(theme::ERROR_MAROON, err);
                }
                Some(PageContent::Site(page)) => {
                    ui.heading(&page.title);
                    ui.separator();
                    ui.label(&page.text);
                    if !page.links.is_empty() {
                        ui.add_space(8.0);
                        ui.separator();
                        ui.label("Links on this page:");
                        for link in &page.links {
                            if ui.link(link).clicked() {
                                clicked_link = Some(link.clone());
                            }
                        }
                    }
                }
                Some(PageContent::SearchResults { query, hits }) => {
                    ui.heading(format!("Search: {query}"));
                    ui.separator();
                    if hits.is_empty() {
                        ui.label("No results.");
                    } else {
                        for hit in hits {
                            ui.add_space(6.0);
                            let label = if hit.title.is_empty() { hit.domain.clone() } else { hit.title.clone() };
                            let target = format!("{}{}", hit.domain, hit.path);
                            if ui.link(format!("{label}  ({target})")).clicked() {
                                clicked_link = Some(target);
                            }
                            if !hit.description.is_empty() {
                                ui.label(&hit.description);
                            }
                        }
                    }
                }
            });
        if let Some(link) = clicked_link {
            self.navigate(app, link);
        }
    }
}

enum ToolbarAction {
    Back,
    Forward,
    Refresh,
    Home,
    Go,
}
