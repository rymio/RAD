use crate::app::{AppContext, RymApp};
use crate::theme;
use crate::vfs::VirtualComputer;
use eframe::egui;

enum ClipMode {
    Copy,
    Cut,
}

enum Pending {
    NewFolder(String),
    NewFile(String),
    Rename { old_name: String, new_name: String },
    ConfirmDelete(String),
}

pub struct FileManagerApp {
    current_dir: String,
    selected: Option<String>,
    pending: Option<Pending>,
    clipboard: Option<(String, ClipMode)>,
    error: Option<String>,
}

impl FileManagerApp {
    pub fn new() -> Self {
        Self::at("/")
    }

    /// A File Manager opened on `dir` (e.g. `/Downloads` after files were dropped in).
    pub fn at(dir: &str) -> Self {
        Self {
            current_dir: dir.to_string(),
            selected: None,
            pending: None,
            clipboard: None,
            error: None,
        }
    }

    /// The heap address of this instance, stable for its lifetime even if
    /// the `Vec<Box<dyn RymApp>>` holding it reallocates -- used to scope
    /// every egui id this app creates so two open File Manager windows
    /// never collide with each other.
    fn instance_id(&self) -> usize {
        self as *const Self as usize
    }

    fn join(&self, name: &str) -> String {
        if self.current_dir.ends_with('/') {
            format!("{}{name}", self.current_dir)
        } else {
            format!("{}/{name}", self.current_dir)
        }
    }

    fn navigate_up(&mut self) {
        if self.current_dir == "/" {
            return;
        }
        let trimmed = self.current_dir.trim_end_matches('/');
        self.current_dir = match trimmed.rfind('/') {
            Some(0) | None => "/".to_string(),
            Some(idx) => trimmed[..idx].to_string(),
        };
        self.selected = None;
    }

    fn paste(&mut self, vfs: &VirtualComputer) {
        let Some((source, mode)) = self.clipboard.take() else {
            return;
        };
        let name = source.rsplit('/').next().unwrap_or(&source).to_string();
        let dest = self.join(&name);
        let result = match mode {
            ClipMode::Copy => vfs.cp(&source, &dest),
            ClipMode::Cut => vfs.mv(&source, &dest),
        };
        if let Err(err) = result {
            self.error = Some(format!("error: {err:#}"));
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, vfs: &VirtualComputer) {
        ui.horizontal(|ui| {
            let at_root = self.current_dir == "/";
            if ui.add_enabled(!at_root, egui::Button::new("\u{2b06} Up")).clicked() {
                self.navigate_up();
            }
            ui.separator();
            if ui.button("\u{1f5c0}+ New Folder").clicked() {
                self.pending = Some(Pending::NewFolder(String::new()));
            }
            if ui.button("\u{1f5cb}+ New File").clicked() {
                self.pending = Some(Pending::NewFile(String::new()));
            }
            let has_selection = self.selected.is_some();
            if ui.add_enabled(has_selection, egui::Button::new("\u{270e} Rename")).clicked()
                && let Some(name) = self.selected.clone()
            {
                self.pending = Some(Pending::Rename {
                    old_name: name.clone(),
                    new_name: name,
                });
            }
            if ui.add_enabled(has_selection, egui::Button::new("\u{1f5d1} Delete")).clicked()
                && let Some(name) = self.selected.clone()
            {
                self.pending = Some(Pending::ConfirmDelete(name));
            }
            ui.separator();
            if ui.add_enabled(has_selection, egui::Button::new("\u{29c9} Copy")).clicked()
                && let Some(name) = &self.selected
            {
                self.clipboard = Some((self.join(name), ClipMode::Copy));
            }
            if ui.add_enabled(has_selection, egui::Button::new("\u{2702} Cut")).clicked()
                && let Some(name) = &self.selected
            {
                self.clipboard = Some((self.join(name), ClipMode::Cut));
            }
            if ui
                .add_enabled(self.clipboard.is_some(), egui::Button::new("\u{1f4cb} Paste"))
                .clicked()
            {
                self.paste(vfs);
            }
        });
        ui.label(format!("Location: {}", self.current_dir));
        ui.separator();
    }

    fn tree_pane(&mut self, ui: &mut egui::Ui, vfs: &VirtualComputer) {
        egui::ScrollArea::vertical()
            .id_salt(("fm_tree_scroll", self.instance_id()))
            .show(ui, |ui| {
                self.tree_node(ui, vfs, "/".to_string(), 0);
            });
    }

    fn tree_node(&mut self, ui: &mut egui::Ui, vfs: &VirtualComputer, path: String, depth: usize) {
        if depth > 8 {
            return;
        }
        let label = if path == "/" {
            "/ (root)".to_string()
        } else {
            path.rsplit('/').next().unwrap_or(&path).to_string()
        };
        let is_current = self.current_dir == path;
        ui.horizontal(|ui| {
            ui.add_space(depth as f32 * 12.0);
            if ui.selectable_label(is_current, format!("\u{1f4c1} {label}")).clicked() {
                self.current_dir = path.clone();
                self.selected = None;
            }
        });
        if let Ok(entries) = vfs.list_entries(&path) {
            for entry in entries.into_iter().filter(|e| e.is_dir) {
                let child_path = if path.ends_with('/') {
                    format!("{path}{}", entry.name)
                } else {
                    format!("{path}/{}", entry.name)
                };
                self.tree_node(ui, vfs, child_path, depth + 1);
            }
        }
    }

    fn list_pane(&mut self, ui: &mut egui::Ui, vfs: &VirtualComputer) {
        egui::ScrollArea::vertical()
            .id_salt(("fm_list_scroll", self.instance_id()))
            .show(ui, |ui| match vfs.list_entries(&self.current_dir) {
                Ok(entries) if entries.is_empty() => {
                    ui.weak("(empty)");
                }
                Ok(entries) => {
                    for entry in entries {
                        let is_selected = self.selected.as_deref() == Some(entry.name.as_str());
                        let icon = if entry.is_dir { "\u{1f4c1}" } else { "\u{1f4c4}" };
                        let label = if entry.is_dir {
                            format!("{icon} {}", entry.name)
                        } else {
                            format!("{icon} {}  ({} bytes)", entry.name, entry.size)
                        };
                        let response = ui.selectable_label(is_selected, label);
                        if response.clicked() {
                            self.selected = Some(entry.name.clone());
                        }
                        if response.double_clicked() && entry.is_dir {
                            self.current_dir = self.join(&entry.name);
                            self.selected = None;
                        }
                    }
                }
                Err(err) => {
                    ui.colored_label(theme::ERROR_MAROON, format!("error: {err:#}"));
                }
            });
    }

    fn pending_dialog(&mut self, ctx: &egui::Context, vfs: &VirtualComputer) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        let instance_id = self.instance_id();
        let mut keep_open = true;
        let mut result = None;

        match pending {
            Pending::NewFolder(mut name) => {
                egui::Window::new("New Folder")
                    .id(egui::Id::new(("fm_new_folder", instance_id)))
                    .collapsible(false)
                    .resizable(false)
                    .show(ctx, |ui| {
                        ui.text_edit_singleline(&mut name);
                        ui.horizontal(|ui| {
                            if ui.button("OK").clicked() && !name.trim().is_empty() {
                                if let Err(err) = vfs.mkdir(&self.join(name.trim())) {
                                    self.error = Some(format!("error: {err:#}"));
                                }
                                keep_open = false;
                            }
                            if ui.button("Cancel").clicked() {
                                keep_open = false;
                            }
                        });
                    });
                if keep_open {
                    result = Some(Pending::NewFolder(name));
                }
            }
            Pending::NewFile(mut name) => {
                egui::Window::new("New File")
                    .id(egui::Id::new(("fm_new_file", instance_id)))
                    .collapsible(false)
                    .resizable(false)
                    .show(ctx, |ui| {
                        ui.text_edit_singleline(&mut name);
                        ui.horizontal(|ui| {
                            if ui.button("OK").clicked() && !name.trim().is_empty() {
                                if let Err(err) = vfs.touch(&self.join(name.trim())) {
                                    self.error = Some(format!("error: {err:#}"));
                                }
                                keep_open = false;
                            }
                            if ui.button("Cancel").clicked() {
                                keep_open = false;
                            }
                        });
                    });
                if keep_open {
                    result = Some(Pending::NewFile(name));
                }
            }
            Pending::Rename { old_name, mut new_name } => {
                egui::Window::new("Rename")
                    .id(egui::Id::new(("fm_rename", instance_id)))
                    .collapsible(false)
                    .resizable(false)
                    .show(ctx, |ui| {
                        ui.text_edit_singleline(&mut new_name);
                        ui.horizontal(|ui| {
                            let changed = !new_name.trim().is_empty() && new_name.trim() != old_name;
                            if ui.add_enabled(changed, egui::Button::new("OK")).clicked() {
                                let dest = new_name.trim().to_string();
                                if let Err(err) = vfs.mv(&self.join(&old_name), &self.join(&dest)) {
                                    self.error = Some(format!("error: {err:#}"));
                                } else {
                                    self.selected = Some(dest);
                                }
                                keep_open = false;
                            }
                            if ui.button("Cancel").clicked() {
                                keep_open = false;
                            }
                        });
                    });
                if keep_open {
                    result = Some(Pending::Rename { old_name, new_name });
                }
            }
            Pending::ConfirmDelete(name) => {
                egui::Window::new("Delete")
                    .id(egui::Id::new(("fm_delete", instance_id)))
                    .collapsible(false)
                    .resizable(false)
                    .show(ctx, |ui| {
                        ui.label(format!("Delete '{name}'? This cannot be undone."));
                        ui.horizontal(|ui| {
                            if ui.button("Delete").clicked() {
                                if let Err(err) = vfs.rm(&self.join(&name), true) {
                                    self.error = Some(format!("error: {err:#}"));
                                }
                                self.selected = None;
                                keep_open = false;
                            }
                            if ui.button("Cancel").clicked() {
                                keep_open = false;
                            }
                        });
                    });
                if keep_open {
                    result = Some(Pending::ConfirmDelete(name));
                }
            }
        }

        self.pending = result;
    }
}

impl RymApp for FileManagerApp {
    fn title(&self) -> String {
        "File Manager".to_string()
    }

    fn show(&mut self, ctx: &egui::Context, app: &AppContext, _launch: &mut Vec<Box<dyn RymApp>>) -> bool {
        let vfs = app.vfs;
        let mut open = true;
        theme::DecoratedWindow::new(egui::Id::new(("file_manager_window", self.instance_id())), self.title())
            .icon("\u{1f4c1}")
            .default_size([520.0, 380.0])
            .show(ctx, &mut open, |ui| {
                self.toolbar(ui, vfs);
                egui::Panel::left(egui::Id::new(("fm_tree_panel", self.instance_id())))
                    .default_size(160.0)
                    .show(ui, |ui| {
                        self.tree_pane(ui, vfs);
                    });
                self.list_pane(ui, vfs);

                if let Some(err) = self.error.clone() {
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.colored_label(theme::ERROR_MAROON, &err);
                        if ui.small_button("x").clicked() {
                            self.error = None;
                        }
                    });
                }
            });

        self.pending_dialog(ctx, vfs);
        open
    }
}
