use crate::app::{AppContext, RymApp};
use crate::rymscript::{EventValue, Widget, WinAppRunner};
use crate::theme;
use eframe::egui;
use std::collections::HashMap;
use std::path::PathBuf;

/// Hosts one running `.win` RymScript program as a desktop window. The
/// interpreter (`WinAppRunner`) is loaded once and kept alive for as long
/// as this window is open, so the script's own globals persist across
/// frames the way a normal running program's state would.
pub struct WinAppWindow {
    title: String,
    runner: Result<WinAppRunner, String>,
    events: HashMap<String, EventValue>,
    text_state: HashMap<String, String>,
    bool_state: HashMap<String, bool>,
    frame_error: Option<String>,
}

impl WinAppWindow {
    pub fn new(title: String, source: &str, root: PathBuf) -> Self {
        let runner = WinAppRunner::load(source, root).map_err(|err| format!("{err:#}"));
        Self {
            title,
            runner,
            events: HashMap::new(),
            text_state: HashMap::new(),
            bool_state: HashMap::new(),
            frame_error: None,
        }
    }

    fn instance_id(&self) -> usize {
        self as *const Self as usize
    }
}

impl RymApp for WinAppWindow {
    fn title(&self) -> String {
        self.title.clone()
    }

    fn show(&mut self, ctx: &egui::Context, _app: &AppContext, _launch: &mut Vec<Box<dyn RymApp>>) -> bool {
        let mut open = true;

        let runner = match &self.runner {
            Ok(runner) => runner,
            Err(err) => {
                theme::DecoratedWindow::new(egui::Id::new(("win_app_error", self.instance_id())), self.title.clone())
                    .show(ctx, &mut open, |ui| {
                        ui.colored_label(theme::ERROR_MAROON, format!("failed to load: {err}"));
                    });
                return open;
            }
        };

        let widgets = match runner.frame(&self.events) {
            Ok(widgets) => {
                self.frame_error = None;
                widgets
            }
            Err(err) => {
                self.frame_error = Some(format!("{err:#}"));
                Vec::new()
            }
        };

        let mut next_events = HashMap::new();
        theme::DecoratedWindow::new(egui::Id::new(("win_app", self.instance_id())), self.title.clone())
            .default_size([360.0, 300.0])
            .show(ctx, &mut open, |ui| {
                for widget in &widgets {
                    render_widget(ui, widget, &mut next_events, &mut self.text_state, &mut self.bool_state);
                }
                if let Some(err) = &self.frame_error {
                    ui.separator();
                    ui.colored_label(theme::ERROR_MAROON, err);
                }
            });

        self.events = next_events;
        // Without this, a `.win` window only ever redraws in response to
        // user input (mouse move, click, typing) -- fine for a form, not
        // for anything that wants to show live-changing state (a clock's
        // `now()`, a Minesweeper timer) while nobody's touching it. Same
        // pattern `ircapp.rs`/`boardapp.rs` already use for their own
        // timer-driven repaints.
        ctx.request_repaint_after(std::time::Duration::from_secs(1));
        open
    }
}

/// Renders one widget (recursing into `Row`/`Column`/`Grid` children) and
/// records any interaction into `next_events`, the same accumulator every
/// leaf widget already wrote into before this was extracted out of the
/// window's `show()` closure to make recursion possible.
fn render_widget(
    ui: &mut egui::Ui,
    widget: &Widget,
    next_events: &mut HashMap<String, EventValue>,
    text_state: &mut HashMap<String, String>,
    bool_state: &mut HashMap<String, bool>,
) {
    match widget {
        Widget::Heading(text) => {
            ui.heading(text);
        }
        Widget::Label(text) => {
            ui.label(text);
        }
        Widget::Separator => {
            ui.separator();
        }
        Widget::Button { id, text, color, enabled } => {
            let mut button = egui::Button::new(text.as_str());
            if let Some([r, g, b]) = color {
                button = button.fill(egui::Color32::from_rgb(*r, *g, *b));
            }
            if ui.add_enabled(*enabled, button).clicked() {
                next_events.insert(id.clone(), EventValue::Clicked);
            }
        }
        Widget::TextInput { id, value } => {
            let stored = text_state.entry(id.clone()).or_insert_with(|| value.clone());
            ui.text_edit_singleline(stored);
            next_events.insert(id.clone(), EventValue::Text(stored.clone()));
        }
        Widget::Checkbox { id, text, value } => {
            let stored = bool_state.entry(id.clone()).or_insert(*value);
            ui.checkbox(stored, text);
            next_events.insert(id.clone(), EventValue::Bool(*stored));
        }
        Widget::Row(children) => {
            ui.horizontal(|ui| {
                for child in children {
                    render_widget(ui, child, next_events, text_state, bool_state);
                }
            });
        }
        Widget::Column(children) => {
            ui.vertical(|ui| {
                for child in children {
                    render_widget(ui, child, next_events, text_state, bool_state);
                }
            });
        }
        Widget::Grid { id, columns, children } => {
            egui::Grid::new(id.as_str()).num_columns(*columns).show(ui, |ui| {
                for (i, child) in children.iter().enumerate() {
                    render_widget(ui, child, next_events, text_state, bool_state);
                    if (i + 1) % columns == 0 {
                        ui.end_row();
                    }
                }
            });
        }
    }
}
