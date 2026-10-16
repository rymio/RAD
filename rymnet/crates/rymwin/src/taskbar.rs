use crate::theme;
use eframe::egui;

const START_MENU_OPEN: &str = "rymwin.start_menu_open";

pub enum StartAction {
    FileManager,
    Rymcode,
    Browser,
    Terminal,
    Mail,
    Irc,
    PdfReader,
    Paint,
    Writer,
    Board,
}

/// The persistent bottom taskbar, shown on every screen (login included --
/// it's meant to read as fixed OS chrome, not something that only appears
/// once you're "inside" something). `logged_in` gates whether the Start
/// menu's app entries are actually clickable: there's no vfs to browse
/// before login, so they're visible but disabled there.
pub fn show(ui: &mut egui::Ui, logged_in: bool) -> Option<StartAction> {
    let mut action = None;
    let panel = egui::Panel::bottom("taskbar")
        .frame(
            egui::Frame::default()
                .fill(theme::FACE_GRAY)
                .inner_margin(egui::Margin::symmetric(4, 3)),
        )
        .exact_size(theme::TASKBAR_HEIGHT)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let start = ui.add(egui::Button::new("\u{25a3} Start"));
                if start.clicked() {
                    toggle_start_menu(ui);
                }
                if start_menu_open(ui) {
                    action = show_start_menu(ui, &start, logged_in);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    show_clock(ui);
                });
            });
        });
    theme::bevel_box(ui, panel.response.rect, true);
    action
}

/// The clock's label text only changes once a second, but egui doesn't
/// repaint on a timer by itself -- without explicitly requesting one, the
/// clock would sit frozen until the next unrelated redraw (e.g. a mouse
/// move over the window).
fn show_clock(ui: &mut egui::Ui) {
    let now = chrono::Local::now();
    let inner = egui::Frame::default()
        .inner_margin(egui::Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.label(now.format("%m/%d/%Y  %I:%M:%S %p").to_string());
        });
    theme::bevel_box(ui, inner.response.rect, false);
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
}

fn start_menu_open(ui: &egui::Ui) -> bool {
    ui.data(|d| d.get_temp(egui::Id::new(START_MENU_OPEN)).unwrap_or(false))
}

fn toggle_start_menu(ui: &egui::Ui) {
    let open = start_menu_open(ui);
    ui.data_mut(|d| d.insert_temp(egui::Id::new(START_MENU_OPEN), !open));
}

fn close_start_menu(ui: &egui::Ui) {
    ui.data_mut(|d| d.insert_temp(egui::Id::new(START_MENU_OPEN), false));
}

fn show_start_menu(ui: &mut egui::Ui, anchor: &egui::Response, logged_in: bool) -> Option<StartAction> {
    let mut action = None;
    let popup_id = egui::Id::new("rymwin.start_menu_popup");
    // Anchored above the taskbar by an estimate of this menu's height
    // (ten rows plus frame margins) -- bump this if more entries are
    // added.
    let response = egui::Area::new(popup_id)
        .fixed_pos(anchor.rect.left_top() - egui::vec2(0.0, 276.0))
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            let frame = egui::Frame::default()
                .fill(theme::FACE_GRAY)
                .inner_margin(egui::Margin::same(4));
            let inner = frame.show(ui, |ui| {
                ui.set_width(180.0);
                // Every entry uses `min_size` pinned to the full menu
                // width instead of a plain `egui::Button::new(label)` --
                // buttons otherwise size to fit their own text, which
                // left-aligned into a ragged menu once labels ranged from
                // "Mail" to "File Manager (log in first)".
                let full_width = egui::vec2(ui.available_width(), 0.0);
                let fm_label = if logged_in { "File Manager" } else { "File Manager (log in first)" };
                if ui.add_enabled(logged_in, egui::Button::new(fm_label).min_size(full_width)).clicked() {
                    action = Some(StartAction::FileManager);
                    close_start_menu(ui);
                }
                let code_label = if logged_in { "Rymcode" } else { "Rymcode (log in first)" };
                if ui.add_enabled(logged_in, egui::Button::new(code_label).min_size(full_width)).clicked() {
                    action = Some(StartAction::Rymcode);
                    close_start_menu(ui);
                }
                // Browsing itself doesn't need an account (peer-protocol
                // fetches, not vfs access) -- it's gated the same as the
                // other two here only because there's no app-hosting
                // surface before the desktop screen exists, not because
                // of what it actually needs.
                let browser_label = if logged_in { "Browser" } else { "Browser (log in first)" };
                if ui.add_enabled(logged_in, egui::Button::new(browser_label).min_size(full_width)).clicked() {
                    action = Some(StartAction::Browser);
                    close_start_menu(ui);
                }
                let terminal_label = if logged_in { "Terminal" } else { "Terminal (log in first)" };
                if ui.add_enabled(logged_in, egui::Button::new(terminal_label).min_size(full_width)).clicked() {
                    action = Some(StartAction::Terminal);
                    close_start_menu(ui);
                }
                let mail_label = if logged_in { "Mail" } else { "Mail (log in first)" };
                if ui.add_enabled(logged_in, egui::Button::new(mail_label).min_size(full_width)).clicked() {
                    action = Some(StartAction::Mail);
                    close_start_menu(ui);
                }
                let irc_label = if logged_in { "Chat" } else { "Chat (log in first)" };
                if ui.add_enabled(logged_in, egui::Button::new(irc_label).min_size(full_width)).clicked() {
                    action = Some(StartAction::Irc);
                    close_start_menu(ui);
                }
                let pdf_label = if logged_in { "PDF Reader" } else { "PDF Reader (log in first)" };
                if ui.add_enabled(logged_in, egui::Button::new(pdf_label).min_size(full_width)).clicked() {
                    action = Some(StartAction::PdfReader);
                    close_start_menu(ui);
                }
                let paint_label = if logged_in { "Paint" } else { "Paint (log in first)" };
                if ui.add_enabled(logged_in, egui::Button::new(paint_label).min_size(full_width)).clicked() {
                    action = Some(StartAction::Paint);
                    close_start_menu(ui);
                }
                let writer_label = if logged_in { "Writer" } else { "Writer (log in first)" };
                if ui.add_enabled(logged_in, egui::Button::new(writer_label).min_size(full_width)).clicked() {
                    action = Some(StartAction::Writer);
                    close_start_menu(ui);
                }
                let board_label = if logged_in { "Board" } else { "Board (log in first)" };
                if ui.add_enabled(logged_in, egui::Button::new(board_label).min_size(full_width)).clicked() {
                    action = Some(StartAction::Board);
                    close_start_menu(ui);
                }
            });
            theme::bevel_box(ui, inner.response.rect, true);
        })
        .response;

    // Click-away-to-close: anything outside the popup or the Start button
    // itself dismisses it, same as a real Start menu.
    if action.is_none()
        && ui.ctx().input(|i| i.pointer.any_click())
        && !response.rect.contains(ui.ctx().pointer_interact_pos().unwrap_or_default())
        && !anchor.rect.contains(ui.ctx().pointer_interact_pos().unwrap_or_default())
    {
        close_start_menu(ui);
    }

    action
}
