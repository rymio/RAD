use crate::app::{AppContext, RymApp};
use crate::designer::{self, WidgetPath};
use crate::rymscript::{Widget, WinAppRunner, build_interpreter, format_exception};
use crate::theme;
use crate::vfs::VirtualComputer;
use crate::winapp::WinAppWindow;
use eframe::egui;
use egui::text::{CCursor, CCursorRange};
use egui::widgets::text_edit::TextEditState;
use rustpython_vm::compiler;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// Code view (the plain text editor this app always had) or Design view
/// (the visual form designer -- see `designer.rs`'s module doc for the
/// generated-layout-block convention that makes it possible without
/// clobbering hand-written code). Only offered for `.win` files; `.run`
/// files have no widget tree to design against.
#[derive(Debug, PartialEq, Clone, Copy)]
enum EditorView {
    Code,
    Design,
}

struct NewFileState {
    name: String,
    blank_form: bool,
}

/// A deliberately small editor: one file open at a time, a file tree to
/// switch between files, Save, and Run. "Clone of VSCode" per the brief,
/// but the "super simple" qualifier is doing a lot of work -- no tabs, no
/// syntax highlighting, no multi-pane splits. Those are cheap to add
/// later on top of this once the single-file flow is solid.
pub struct RymcodeApp {
    open_file: Option<String>,
    buffer: String,
    dirty: bool,
    pending_new_file: Option<NewFileState>,
    output: Option<String>,
    error: Option<String>,
    view: EditorView,
    /// The parsed widget tree Design view renders/edits -- `None` if the
    /// current buffer failed to load/run (see `design_error`) or Design
    /// view has never been entered for this file yet.
    design_layout: Option<Vec<Widget>>,
    design_error: Option<String>,
    /// Whether the buffer already used the generated-layout-block
    /// convention when Design view was last entered -- gates whether the
    /// Palette/Property Inspector are allowed to edit it at all, so a
    /// hand-written script (calculator.win, say) is only ever shown
    /// read-only, never silently rewritten.
    design_editable: bool,
    selected: Option<WidgetPath>,
    next_widget_id: usize,
    pending_cursor_jump: Option<String>,
}

impl RymcodeApp {
    pub fn new() -> Self {
        Self {
            open_file: None,
            buffer: String::new(),
            dirty: false,
            pending_new_file: None,
            output: None,
            error: None,
            view: EditorView::Code,
            design_layout: None,
            design_error: None,
            design_editable: false,
            selected: None,
            next_widget_id: 0,
            pending_cursor_jump: None,
        }
    }

    fn instance_id(&self) -> usize {
        self as *const Self as usize
    }

    fn code_editor_id(&self) -> egui::Id {
        egui::Id::new(("rymcode_editor", self.instance_id()))
    }

    fn open_path(&mut self, vfs: &VirtualComputer, path: String) {
        match vfs.read_to_string(&path) {
            Ok(text) => {
                self.open_file = Some(path);
                self.buffer = text;
                self.dirty = false;
                self.output = None;
                self.error = None;
                self.view = EditorView::Code;
                self.design_layout = None;
                self.design_error = None;
                self.selected = None;
            }
            Err(err) => self.error = Some(format!("error opening {path}: {err:#}")),
        }
    }

    fn save(&mut self, vfs: &VirtualComputer) {
        let Some(path) = self.open_file.clone() else {
            return;
        };
        match vfs.write_string(&path, &self.buffer) {
            Ok(()) => self.dirty = false,
            Err(err) => self.error = Some(format!("error saving {path}: {err:#}")),
        }
    }

    fn tree_pane(&mut self, ui: &mut egui::Ui, vfs: &VirtualComputer) {
        egui::ScrollArea::vertical()
            .id_salt(("rymcode_tree", self.instance_id()))
            .show(ui, |ui| {
                self.tree_node(ui, vfs, "/".to_string(), 0);
            });
    }

    fn tree_node(&mut self, ui: &mut egui::Ui, vfs: &VirtualComputer, path: String, depth: usize) {
        if depth > 8 {
            return;
        }
        let Ok(entries) = vfs.list_entries(&path) else {
            return;
        };
        for entry in entries {
            let child_path = if path.ends_with('/') {
                format!("{path}{}", entry.name)
            } else {
                format!("{path}/{}", entry.name)
            };
            ui.horizontal(|ui| {
                ui.add_space(depth as f32 * 12.0);
                if entry.is_dir {
                    ui.label(format!("\u{1f4c1} {}", entry.name));
                } else {
                    let is_open = self.open_file.as_deref() == Some(child_path.as_str());
                    if ui.selectable_label(is_open, format!("\u{1f4c4} {}", entry.name)).clicked() {
                        self.open_path(vfs, child_path.clone());
                    }
                }
            });
            if entry.is_dir {
                self.tree_node(ui, vfs, child_path, depth + 1);
            }
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, vfs: &VirtualComputer, launch: &mut Vec<Box<dyn RymApp>>) {
        ui.horizontal(|ui| {
            if ui.button("\u{1f5cb}+ New File").clicked() {
                self.pending_new_file = Some(NewFileState { name: String::new(), blank_form: false });
            }
            let has_file = self.open_file.is_some();
            if ui
                .add_enabled(has_file && self.dirty, egui::Button::new("\u{1f4be} Save"))
                .clicked()
            {
                self.save(vfs);
            }
            if ui.add_enabled(has_file, egui::Button::new("\u{25b6} Run")).clicked() {
                self.run(vfs, launch);
            }
            let is_win_file = self.open_file.as_deref().is_some_and(|path| path.ends_with(".win"));
            if is_win_file {
                ui.separator();
                if ui.selectable_label(self.view == EditorView::Code, "Code").clicked() {
                    self.view = EditorView::Code;
                }
                if ui.selectable_label(self.view == EditorView::Design, "Design").clicked() {
                    self.switch_to_design(vfs);
                }
            }
            if let Some(path) = &self.open_file {
                ui.label(format!("{path}{}", if self.dirty { " *" } else { "" }));
            }
        });
        ui.separator();
    }

    /// Loads and runs the current buffer through the exact same
    /// interpreter path a live `.win` app uses (`WinAppRunner::load` +
    /// one `.frame()` call) to get the widget tree Design view renders --
    /// reused rather than reinvented, the same way `WinAppWindow` already
    /// does for actually running one of these programs.
    fn switch_to_design(&mut self, vfs: &VirtualComputer) {
        self.view = EditorView::Design;
        self.selected = None;
        self.design_editable = designer::has_layout_block(&self.buffer);
        match WinAppRunner::load(&self.buffer, vfs.root.clone()).and_then(|runner| runner.frame(&HashMap::new())) {
            Ok(widgets) => {
                self.design_layout = Some(widgets);
                self.design_error = None;
            }
            Err(err) => {
                self.design_layout = None;
                self.design_error = Some(format!("{err:#}"));
            }
        }
    }

    /// Regenerates the marked layout block from `design_layout` and
    /// splices it into `self.buffer` -- see `designer.rs`'s
    /// `replace_layout_block` for why this only ever touches the text
    /// between the two markers. Only called when `design_editable`, so a
    /// read-only (hand-written) file is never rewritten.
    fn sync_design_to_buffer(&mut self) {
        // Checked here too, not just by the caller in `design_view` --
        // this is the one method that can actually rewrite a user's
        // buffer, so it doesn't rely solely on being called correctly.
        if !self.design_editable {
            return;
        }
        let Some(widgets) = &self.design_layout else { return };
        let new_block = designer::generate_layout_block(widgets);
        let updated = designer::replace_layout_block(&self.buffer, &new_block);
        if updated != self.buffer {
            self.buffer = updated;
            self.dirty = true;
        }
    }

    fn add_widget(&mut self, kind: &str) {
        let Some(widgets) = &mut self.design_layout else { return };
        self.next_widget_id += 1;
        let new_widget = designer::default_widget(kind, self.next_widget_id);
        let target_path: Vec<usize> = match &self.selected {
            Some(path) => match designer::widget_at(widgets, path) {
                Some(widget) if designer::is_container(widget) => path.clone(),
                Some(_) => path[..path.len().saturating_sub(1)].to_vec(),
                None => Vec::new(),
            },
            None => Vec::new(),
        };
        if let Some(children) = designer::children_at_mut(widgets, &target_path) {
            children.push(new_widget);
        }
    }

    /// Selects the double-clicked widget, ensures its handler function
    /// exists (creating an empty stub if not), and switches to Code view
    /// with the cursor placed inside it -- the actual "double-click a
    /// widget, write the code that runs for it" moment.
    fn jump_to_handler(&mut self, path: &[usize]) {
        let Some(widgets) = &self.design_layout else { return };
        let Some(widget) = designer::widget_at(widgets, path) else { return };
        let Some(id) = designer::widget_id(widget) else { return };
        let handler_name = match widget {
            Widget::TextInput { .. } | Widget::Checkbox { .. } => format!("on_{id}_change"),
            _ => format!("on_{id}_click"),
        };
        if !self.buffer.contains(&format!("def {handler_name}(")) {
            if !self.buffer.ends_with('\n') {
                self.buffer.push('\n');
            }
            self.buffer.push_str(&format!("\n\ndef {handler_name}(events):\n    pass\n"));
            self.dirty = true;
        }
        self.view = EditorView::Code;
        self.pending_cursor_jump = Some(handler_name);
    }

    /// Sets the Code-view `TextEdit`'s cursor to just inside the handler
    /// function's body -- same `TextEditState`-load/mutate/store pattern
    /// used for this in `writerapp.rs`'s toolbar-driven syntax insertion.
    fn apply_pending_cursor_jump(&mut self, ctx: &egui::Context) {
        let Some(handler_name) = self.pending_cursor_jump.take() else { return };
        let needle = format!("def {handler_name}(events):\n    ");
        let Some(byte_pos) = self.buffer.find(&needle) else { return };
        let char_pos = self.buffer[..byte_pos].chars().count() + needle.chars().count();
        let id = self.code_editor_id();
        let mut state = TextEditState::load(ctx, id).unwrap_or_default();
        state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(char_pos))));
        state.store(ctx, id);
    }

    fn run(&mut self, vfs: &VirtualComputer, launch: &mut Vec<Box<dyn RymApp>>) {
        let Some(path) = self.open_file.clone() else {
            return;
        };
        self.output = None;
        self.error = None;
        if path.ends_with(".win") {
            let title = path.rsplit('/').next().unwrap_or(&path).to_string();
            launch.push(Box::new(WinAppWindow::new(title, &self.buffer, vfs.root.clone())));
        } else if path.ends_with(".run") {
            match run_preview(&self.buffer, vfs.root.clone()) {
                Ok(output) => self.output = Some(output),
                Err(err) => self.error = Some(format!("{err:#}")),
            }
        } else {
            self.error = Some(
                "only *.run and *.win files can be run -- save this file with one of those extensions first"
                    .to_string(),
            );
        }
    }

    fn new_file_dialog(&mut self, ctx: &egui::Context, vfs: &VirtualComputer) {
        let Some(mut state) = self.pending_new_file.take() else {
            return;
        };
        let mut keep_open = true;
        egui::Window::new("New File")
            .id(egui::Id::new(("rymcode_new_file", self.instance_id())))
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label("Create in /Programs -- name.run (terminal) or name.win (desktop):");
                ui.text_edit_singleline(&mut state.name);
                ui.checkbox(&mut state.blank_form, "Blank Form (.win) -- start directly in Design view");
                ui.horizontal(|ui| {
                    if ui.button("Create").clicked() && !state.name.trim().is_empty() {
                        let mut name = state.name.trim().to_string();
                        if state.blank_form && !name.ends_with(".win") {
                            name.push_str(".win");
                        }
                        let path = format!("/Programs/{name}");
                        let contents = if state.blank_form { designer::generate_layout_block(&[]) } else { String::new() };
                        if let Err(err) = vfs.write_string(&path, &contents) {
                            self.error = Some(format!("error: {err:#}"));
                        } else {
                            self.open_path(vfs, path);
                            if state.blank_form {
                                self.switch_to_design(vfs);
                            }
                        }
                        keep_open = false;
                    }
                    if ui.button("Cancel").clicked() {
                        keep_open = false;
                    }
                });
            });
        if keep_open {
            self.pending_new_file = Some(state);
        }
    }

    fn palette(&mut self, ui: &mut egui::Ui) {
        ui.label("Add widget:");
        const KINDS: [&str; 9] = ["heading", "label", "separator", "button", "text_input", "checkbox", "row", "column", "grid"];
        egui::Grid::new(("rymcode_palette", self.instance_id())).num_columns(3).show(ui, |ui| {
            for (i, kind) in KINDS.iter().enumerate() {
                if ui.button(*kind).clicked() {
                    self.add_widget(kind);
                }
                if (i + 1) % 3 == 0 {
                    ui.end_row();
                }
            }
        });
    }

    fn property_inspector(&mut self, ui: &mut egui::Ui) {
        let Some(path) = self.selected.clone() else {
            ui.weak("Select a widget to edit its properties.");
            return;
        };
        let Some(widgets) = &mut self.design_layout else { return };
        let Some(widget) = designer::widget_at_mut(widgets, &path) else {
            self.selected = None;
            return;
        };
        ui.label(designer::design_summary(widget));
        ui.separator();
        match widget {
            Widget::Heading(text) | Widget::Label(text) => {
                ui.label("Text:");
                ui.text_edit_multiline(text);
            }
            Widget::Separator => {}
            Widget::Button { id, text, color, enabled } => {
                ui.label("Id:");
                ui.text_edit_singleline(id);
                ui.label("Text:");
                ui.text_edit_singleline(text);
                ui.checkbox(enabled, "Enabled");
                let mut has_color = color.is_some();
                ui.checkbox(&mut has_color, "Custom color");
                if has_color {
                    let [r, g, b] = color.unwrap_or([200, 200, 200]);
                    let mut picked = egui::Color32::from_rgb(r, g, b);
                    ui.color_edit_button_srgba(&mut picked);
                    *color = Some([picked.r(), picked.g(), picked.b()]);
                } else {
                    *color = None;
                }
            }
            Widget::TextInput { id, value } => {
                ui.label("Id:");
                ui.text_edit_singleline(id);
                ui.label("Initial value:");
                ui.text_edit_singleline(value);
            }
            Widget::Checkbox { id, text, value } => {
                ui.label("Id:");
                ui.text_edit_singleline(id);
                ui.label("Text:");
                ui.text_edit_singleline(text);
                ui.checkbox(value, "Initial value");
            }
            Widget::Row(_) | Widget::Column(_) => {
                ui.weak("Layout container -- select it and add widgets from the palette to place them inside.");
            }
            Widget::Grid { id, columns, .. } => {
                ui.label("Id:");
                ui.text_edit_singleline(id);
                ui.label("Columns:");
                ui.add(egui::DragValue::new(columns).range(1..=12));
            }
        }
        ui.separator();
        if ui.button("\u{1f5d1} Delete").clicked() {
            designer::remove_widget(widgets, &path);
            self.selected = None;
        }
    }

    fn design_view(&mut self, ui: &mut egui::Ui) {
        if let Some(err) = &self.design_error {
            ui.colored_label(theme::ERROR_MAROON, format!("Can't render Design view: {err}"));
            return;
        }
        if !self.design_editable {
            ui.weak(
                "This file doesn't use the visual layout format yet -- it's shown read-only here. \
                 Start a new file with \"Blank Form (.win)\" to build one visually, or keep editing this one in Code view.",
            );
        }

        egui::Panel::right(egui::Id::new(("rymcode_inspector_panel", self.instance_id())))
            .default_size(220.0)
            .show(ui, |ui| {
                if self.design_editable {
                    self.palette(ui);
                    ui.separator();
                }
                self.property_inspector(ui);
            });

        let mut clicked = None;
        let mut double_clicked = None;
        egui::ScrollArea::vertical().id_salt(("rymcode_design_canvas", self.instance_id())).show(ui, |ui| {
            if let Some(widgets) = &self.design_layout {
                for (i, widget) in widgets.iter().enumerate() {
                    render_design_widget(ui, widget, &[i], &self.selected, &mut clicked, &mut double_clicked);
                }
            } else {
                ui.weak("Nothing to show.");
            }
        });

        if let Some(path) = double_clicked {
            self.selected = Some(path.clone());
            if self.design_editable {
                self.jump_to_handler(&path);
            }
        } else if let Some(path) = clicked {
            self.selected = Some(path);
        }

        if self.design_editable {
            self.sync_design_to_buffer();
        }
    }
}

/// Renders one row of Design view's component tree (recursing into
/// containers, indented), reporting a click/double-click on `path` via
/// the two accumulators rather than mutating app state directly -- kept
/// as a free function since it only ever needs a read-only view of the
/// tree, the same "accumulate into an out-parameter" shape
/// `winapp.rs`'s own `render_widget` already uses for a similar reason.
fn render_design_widget(
    ui: &mut egui::Ui,
    widget: &Widget,
    path: &[usize],
    selected: &Option<WidgetPath>,
    clicked: &mut Option<WidgetPath>,
    double_clicked: &mut Option<WidgetPath>,
) {
    let is_selected = selected.as_deref() == Some(path);
    let response = ui.selectable_label(is_selected, designer::design_summary(widget));
    if response.double_clicked() {
        *double_clicked = Some(path.to_vec());
    } else if response.clicked() {
        *clicked = Some(path.to_vec());
    }
    if let Some(children) = designer::children_of(widget) {
        ui.indent(("rymcode_design_indent", path.to_vec()), |ui| {
            for (i, child) in children.iter().enumerate() {
                let mut child_path = path.to_vec();
                child_path.push(i);
                render_design_widget(ui, child, &child_path, selected, clicked, double_clicked);
            }
        });
    }
}

impl RymApp for RymcodeApp {
    fn title(&self) -> String {
        "Rymcode".to_string()
    }

    fn show(&mut self, ctx: &egui::Context, app: &AppContext, launch: &mut Vec<Box<dyn RymApp>>) -> bool {
        let vfs = app.vfs;
        let mut open = true;
        theme::DecoratedWindow::new(egui::Id::new(("rymcode_window", self.instance_id())), self.title())
            .icon("\u{1f4dd}")
            .default_size([680.0, 480.0])
            .show(ctx, &mut open, |ui| {
                self.toolbar(ui, vfs, launch);
                egui::Panel::left(egui::Id::new(("rymcode_tree_panel", self.instance_id())))
                    .default_size(180.0)
                    .show(ui, |ui| {
                        self.tree_pane(ui, vfs);
                    });

                match self.view {
                    EditorView::Design => self.design_view(ui),
                    EditorView::Code => {
                        let available = ui.available_height();
                        let output_height = if self.output.is_some() || self.error.is_some() { 90.0 } else { 0.0 };
                        let editor_id = self.code_editor_id();
                        egui::ScrollArea::vertical()
                            .id_salt(("rymcode_editor_scroll", self.instance_id()))
                            .max_height((available - output_height).max(60.0))
                            .show(ui, |ui| {
                                let editor = egui::TextEdit::multiline(&mut self.buffer)
                                    .id(editor_id)
                                    .code_editor()
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(20);
                                if ui.add_enabled(self.open_file.is_some(), editor).changed() {
                                    self.dirty = true;
                                }
                            });
                        self.apply_pending_cursor_jump(ctx);

                        if let Some(output) = &self.output {
                            ui.separator();
                            ui.label("Output (non-interactive preview -- input() isn't available here; use `rymsh`'s `run` command for interactive programs):");
                            egui::ScrollArea::vertical()
                                .id_salt(("rymcode_output_scroll", self.instance_id()))
                                .max_height(60.0)
                                .show(ui, |ui| {
                                    ui.monospace(output);
                                });
                        }
                        if let Some(err) = &self.error {
                            ui.separator();
                            ui.colored_label(theme::ERROR_MAROON, err);
                        }
                    }
                }
            });

        self.new_file_dialog(ctx, vfs);
        open
    }
}

/// Runs a `.run` script non-interactively for Rymcode's quick preview:
/// `print()` output is captured into a string instead of going anywhere
/// real, and `input()` raises immediately rather than hanging a GUI app
/// waiting for a terminal that doesn't exist here.
fn run_preview(source: &str, root: std::path::PathBuf) -> anyhow::Result<String> {
    let interpreter = build_interpreter();
    interpreter.enter(|vm| -> anyhow::Result<String> {
        let scope = vm.new_scope_with_builtins();
        let output = Rc::new(RefCell::new(String::new()));

        {
            let output = output.clone();
            scope
                .globals
                .set_item(
                    "print",
                    vm.new_function("print", move |args: rustpython_vm::function::FuncArgs, vm: &rustpython_vm::VirtualMachine| -> rustpython_vm::PyResult<()> {
                        let mut parts = Vec::new();
                        for arg in &args.args {
                            let py_str = arg.str(vm)?;
                            let text: &str = py_str.as_ref();
                            parts.push(text.to_string());
                        }
                        let mut buf = output.borrow_mut();
                        buf.push_str(&parts.join(" "));
                        buf.push('\n');
                        Ok(())
                    }).into(),
                    vm,
                )
                .map_err(|exc| anyhow::anyhow!(format_exception(vm, exc)))?;
        }
        scope
            .globals
            .set_item(
                "input",
                vm.new_function("input", |_prompt: rustpython_vm::function::OptionalArg<String>, vm: &rustpython_vm::VirtualMachine| -> rustpython_vm::PyResult<String> {
                    Err(vm.new_runtime_error(
                        "input() isn't available in Rymcode's preview -- run this program from rymsh with `run <name>` for interactive input".to_string(),
                    ))
                }).into(),
                vm,
            )
            .map_err(|exc| anyhow::anyhow!(format_exception(vm, exc)))?;

        install_file_helpers(&scope, vm, root)
            .map_err(|exc| anyhow::anyhow!(format_exception(vm, exc)))?;

        let code = vm
            .compile(source, compiler::Mode::Exec, "<rymscript>".to_string())
            .map_err(|err| anyhow::anyhow!("{err}"))?;
        if let Err(exc) = vm.run_code_obj(code, scope.clone()) {
            let mut buf = output.borrow_mut();
            buf.push_str("--- error ---\n");
            buf.push_str(&format_exception(vm, exc));
        }
        Ok(output.borrow().clone())
    })
}

fn install_file_helpers(
    scope: &rustpython_vm::scope::Scope,
    vm: &rustpython_vm::VirtualMachine,
    root: std::path::PathBuf,
) -> rustpython_vm::PyResult<()> {
    use std::fs;
    use std::path::{Component, Path, PathBuf};

    fn sandbox_resolve(root: &Path, input: &str) -> PathBuf {
        let mut result = root.to_path_buf();
        for component in Path::new(input.trim_start_matches('/')).components() {
            match component {
                Component::CurDir => {}
                Component::ParentDir if result.as_path() != root => {
                    result.pop();
                }
                Component::ParentDir => {}
                Component::Normal(part) => result.push(part),
                _ => {}
            }
        }
        result
    }

    macro_rules! set_fn {
        ($name:literal, $func:expr) => {
            scope
                .globals
                .set_item($name, vm.new_function($name, $func).into(), vm)?;
        };
    }

    {
        let root = root.clone();
        set_fn!("read_file", move |path: String, vm: &rustpython_vm::VirtualMachine| -> rustpython_vm::PyResult<String> {
            let real = sandbox_resolve(&root, &path);
            fs::read_to_string(&real).map_err(|err| vm.new_runtime_error(format!("{path}: {err}")))
        });
    }
    {
        let root = root.clone();
        set_fn!("write_file", move |path: String, text: String, vm: &rustpython_vm::VirtualMachine| -> rustpython_vm::PyResult<()> {
            let real = sandbox_resolve(&root, &path);
            if let Some(parent) = real.parent() {
                let _ = fs::create_dir_all(parent);
            }
            fs::write(&real, text).map_err(|err| vm.new_runtime_error(format!("{path}: {err}")))
        });
    }
    {
        let root = root.clone();
        set_fn!("exists", move |path: String| -> bool { sandbox_resolve(&root, &path).exists() });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_vfs() -> VirtualComputer {
        let dir = std::env::temp_dir().join(format!(
            "rymcode-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        VirtualComputer::open(dir).unwrap()
    }

    /// End-to-end: creating a blank form, adding widgets through the
    /// same `add_widget` the palette calls, and confirming the result
    /// both looks right *and* actually loads and runs through the real
    /// interpreter -- not just that `designer.rs`'s functions work in
    /// isolation, but that `RymcodeApp` wires them together correctly.
    #[test]
    fn blank_form_workflow_adds_widgets_and_stays_runnable() {
        let vfs = scratch_vfs();
        let mut app = RymcodeApp::new();

        let path = "/Programs/scratch.win".to_string();
        vfs.write_string(&path, &designer::generate_layout_block(&[])).unwrap();
        app.open_path(&vfs, path);
        app.switch_to_design(&vfs);
        assert!(app.design_error.is_none(), "a freshly generated blank form should load without error");
        assert!(app.design_editable);
        assert_eq!(app.design_layout.as_ref().unwrap().len(), 0);

        app.add_widget("button");
        app.add_widget("button");
        let widgets = app.design_layout.as_ref().unwrap();
        assert_eq!(widgets.len(), 2);
        let Widget::Button { id: first_id, .. } = &widgets[0] else { panic!("expected a Button") };
        let Widget::Button { id: second_id, .. } = &widgets[1] else { panic!("expected a Button") };
        assert_ne!(first_id, second_id, "repeated adds must not collide on id");
        let first_id = first_id.clone();

        app.sync_design_to_buffer();
        assert!(app.buffer.contains(designer::LAYOUT_BEGIN));
        assert!(app.buffer.contains(&first_id));

        // The regenerated buffer must still be valid, loadable RymScript.
        let runner = WinAppRunner::load(&app.buffer, vfs.root.clone()).expect("regenerated buffer should still load");
        let rendered = runner.frame(&HashMap::new()).expect("frame");
        assert_eq!(rendered.len(), 2);

        let _ = std::fs::remove_dir_all(&vfs.root);
    }

    #[test]
    fn double_click_creates_a_handler_stub_and_queues_a_cursor_jump() {
        let vfs = scratch_vfs();
        let mut app = RymcodeApp::new();

        let path = "/Programs/scratch.win".to_string();
        vfs.write_string(&path, &designer::generate_layout_block(&[])).unwrap();
        app.open_path(&vfs, path);
        app.switch_to_design(&vfs);
        app.add_widget("button");
        app.sync_design_to_buffer();

        let widgets = app.design_layout.as_ref().unwrap();
        let Widget::Button { id, .. } = &widgets[0] else { panic!("expected a Button") };
        let id = id.clone();

        app.jump_to_handler(&[0]);
        assert_eq!(app.view, EditorView::Code, "double-click should switch to Code view");
        assert_eq!(app.pending_cursor_jump.as_deref(), Some(format!("on_{id}_click").as_str()));
        assert!(app.buffer.contains(&format!("def on_{id}_click(events):")), "a handler stub should be created");

        // Double-clicking again (e.g. after nothing changed) must not
        // duplicate the stub.
        let stub_count = app.buffer.matches(&format!("def on_{id}_click(")).count();
        app.jump_to_handler(&[0]);
        assert_eq!(app.buffer.matches(&format!("def on_{id}_click(")).count(), stub_count, "re-jumping shouldn't add a second stub");

        let _ = std::fs::remove_dir_all(&vfs.root);
    }

    #[test]
    fn read_only_layout_is_never_rewritten_by_sync() {
        let vfs = scratch_vfs();
        let mut app = RymcodeApp::new();

        // A hand-written script with no marker block at all -- the same
        // shape as calculator.win/minesweeper.win.
        let source = "def draw(events):\n    return [{\"type\": \"label\", \"text\": \"hi\"}]\n";
        let path = "/Programs/handwritten.win".to_string();
        vfs.write_string(&path, source).unwrap();
        app.open_path(&vfs, path);
        app.switch_to_design(&vfs);

        assert!(!app.design_editable, "a file with no marker block must be treated as read-only");
        let before = app.buffer.clone();
        // `sync_design_to_buffer` guards on `design_editable` itself
        // (not just the `design_view` call site), so calling it directly
        // here -- as if that guard were ever accidentally bypassed --
        // must still be a complete no-op.
        app.sync_design_to_buffer();
        assert_eq!(app.buffer, before, "a read-only layout must never be rewritten, byte-for-byte");
        assert!(!app.dirty, "a no-op sync must not mark the file dirty");

        let _ = std::fs::remove_dir_all(&vfs.root);
    }
}
