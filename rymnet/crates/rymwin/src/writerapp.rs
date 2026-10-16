use crate::app::{AppContext, RymApp};
use crate::theme;
use crate::vfs::VirtualComputer;
use eframe::egui;
use egui::text::{CCursor, CCursorRange};
use egui::widgets::text_edit::TextEditState;
use pulldown_cmark::{Event, Options, Parser, Tag, html};

const DEFAULT_STYLE_CSS: &str = r#"body {
    max-width: 780px;
    margin: 2rem auto;
    padding: 0 1rem;
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
    line-height: 1.6;
    color: #222;
}
h1, h2, h3 { line-height: 1.25; }
code, pre { background: #f2f2f2; padding: 0.15em 0.4em; border-radius: 3px; }
pre code { padding: 0; background: none; }
img { max-width: 100%; }
blockquote { margin-left: 0; padding-left: 1em; border-left: 3px solid #ccc; color: #555; }
"#;

#[derive(PartialEq, Clone, Copy)]
enum ViewMode {
    Split,
    SourceOnly,
    PreviewOnly,
}

enum NextAction {
    New,
    Open(String),
}

enum Pending {
    SaveAs { path: String },
    ExportHtml { path: String },
    InsertImage,
    ConfirmDiscard { then: NextAction },
}

/// The ninth desktop app: a markdown editor, extension `.wrr`, with a live
/// rendered preview and one-click HTML(+CSS) export so a document doubles
/// as a `.rym` site source. Markdown is the only source of truth -- there
/// is no invented rich-text format and no click-to-format WYSIWYG, the
/// same "no mature crate exists for that, so don't hand-roll it" call
/// this workspace already made for PDF rendering and PNG editing.
pub struct WriterApp {
    path: Option<String>,
    source: String,
    saved_snapshot: String,
    view_mode: ViewMode,
    preview_cache: egui_commonmark::CommonMarkCache,
    pending: Option<Pending>,
    /// Set by `tree_node` when the Insert Image picker's tree successfully
    /// inserts an image, checked (and reset) by `pending_dialog` right
    /// after, so it knows to close the picker instead of re-arming it.
    image_picked: bool,
    show_tree: bool,
    status: Option<String>,
    error: Option<String>,
}

impl WriterApp {
    pub fn new() -> Self {
        Self {
            path: None,
            source: String::new(),
            saved_snapshot: String::new(),
            view_mode: ViewMode::Split,
            preview_cache: egui_commonmark::CommonMarkCache::default(),
            pending: None,
            image_picked: false,
            show_tree: true,
            status: None,
            error: None,
        }
    }

    fn instance_id(&self) -> usize {
        self as *const Self as usize
    }

    fn source_edit_id(&self) -> egui::Id {
        egui::Id::new(("writer_source", self.instance_id()))
    }

    fn is_dirty(&self) -> bool {
        self.source != self.saved_snapshot
    }

    /// Routes New/Open through a confirm dialog if there are unsaved
    /// changes, otherwise performs the action immediately.
    fn request(&mut self, app: &AppContext, action: NextAction) {
        if self.is_dirty() {
            self.pending = Some(Pending::ConfirmDiscard { then: action });
        } else {
            self.perform(app, action);
        }
    }

    fn perform(&mut self, app: &AppContext, action: NextAction) {
        match action {
            NextAction::New => {
                self.path = None;
                self.source.clear();
                self.saved_snapshot.clear();
                self.status = None;
                self.error = None;
            }
            NextAction::Open(vfs_path) => match app.vfs.read_to_string(&vfs_path) {
                Ok(contents) => {
                    self.path = Some(vfs_path);
                    self.saved_snapshot = contents.clone();
                    self.source = contents;
                    self.status = None;
                    self.error = None;
                }
                Err(err) => self.error = Some(format!("error opening {vfs_path}: {err:#}")),
            },
        }
    }

    fn save(&mut self, app: &AppContext) {
        let Some(path) = self.path.clone() else {
            self.pending = Some(Pending::SaveAs { path: "/untitled.wrr".to_string() });
            return;
        };
        self.save_to(app, path);
    }

    fn save_to(&mut self, app: &AppContext, path: String) {
        match app.vfs.write_string(&path, &self.source) {
            Ok(()) => {
                self.path = Some(path.clone());
                self.saved_snapshot = self.source.clone();
                self.status = Some(format!("saved {path}"));
                self.error = None;
            }
            Err(err) => self.error = Some(format!("error saving {path}: {err:#}")),
        }
    }

    /// Reads the cursor's current character-offset selection out of the
    /// persisted widget state for the source `TextEdit`, if any has been
    /// recorded yet (none on the very first frame before it's ever been
    /// focused).
    fn cursor_range(&self, ctx: &egui::Context) -> Option<CCursorRange> {
        TextEditState::load(ctx, self.source_edit_id()).and_then(|state| state.cursor.char_range())
    }

    /// Splices `prefix`/`suffix` around the current selection (or, if
    /// nothing is selected, inserts `prefix suffix` with the cursor left
    /// in between) and re-focuses the widget with the cursor placed right
    /// after the inserted text. Operates on `Vec<char>`, not raw string
    /// bytes -- `CCursor::index` is a character offset, confirmed in
    /// epaint's own `text::cursor` module, so byte-slicing a `String`
    /// directly would panic or corrupt multi-byte UTF-8 text.
    fn wrap_selection(&mut self, ctx: &egui::Context, prefix: &str, suffix: &str) {
        let range = self.cursor_range(ctx).unwrap_or(CCursorRange::one(CCursor::new(self.source.chars().count())));
        let [start, end] = range.sorted_cursors();
        let mut chars: Vec<char> = self.source.chars().collect();
        let (start_idx, end_idx) = (start.index.0.min(chars.len()), end.index.0.min(chars.len()));

        let prefix_chars: Vec<char> = prefix.chars().collect();
        let suffix_chars: Vec<char> = suffix.chars().collect();
        let selected: Vec<char> = chars[start_idx..end_idx].to_vec();

        let mut replacement = prefix_chars.clone();
        replacement.extend(selected.iter());
        replacement.extend(suffix_chars.iter());
        chars.splice(start_idx..end_idx, replacement);

        self.source = chars.into_iter().collect();

        // With a real selection, land the cursor after the closing suffix
        // (ready to keep typing past the now-wrapped word). With nothing
        // selected, land it between prefix and suffix instead, so the
        // user can type the bold/italic/code word immediately rather than
        // past an empty pair of markers.
        let new_cursor = if selected.is_empty() {
            start_idx + prefix_chars.len()
        } else {
            start_idx + prefix_chars.len() + selected.len() + suffix_chars.len()
        };
        self.set_cursor(ctx, new_cursor);
    }

    /// Inserts `prefix` at the start of the line the primary cursor is on
    /// (e.g. `# ` for a heading, `- ` for a bullet).
    fn prefix_line(&mut self, ctx: &egui::Context, prefix: &str) {
        let range = self.cursor_range(ctx).unwrap_or(CCursorRange::one(CCursor::new(self.source.chars().count())));
        let cursor_idx = range.primary.index.0.min(self.source.chars().count());
        let mut chars: Vec<char> = self.source.chars().collect();

        let mut line_start = cursor_idx;
        while line_start > 0 && chars[line_start - 1] != '\n' {
            line_start -= 1;
        }

        let prefix_chars: Vec<char> = prefix.chars().collect();
        for (offset, ch) in prefix_chars.iter().enumerate() {
            chars.insert(line_start + offset, *ch);
        }

        self.source = chars.into_iter().collect();
        self.set_cursor(ctx, cursor_idx + prefix_chars.len());
    }

    /// Inserts `text` at the primary cursor position (used for the Insert
    /// Image picker, which doesn't need selection-wrapping).
    fn insert_at_cursor(&mut self, ctx: &egui::Context, text: &str) {
        let range = self.cursor_range(ctx).unwrap_or(CCursorRange::one(CCursor::new(self.source.chars().count())));
        let cursor_idx = range.primary.index.0.min(self.source.chars().count());
        let mut chars: Vec<char> = self.source.chars().collect();
        let insert_chars: Vec<char> = text.chars().collect();
        for (offset, ch) in insert_chars.iter().enumerate() {
            chars.insert(cursor_idx + offset, *ch);
        }
        self.source = chars.into_iter().collect();
        self.set_cursor(ctx, cursor_idx + insert_chars.len());
    }

    /// Stores a fresh cursor position (a plain character offset, no
    /// selection) into the source `TextEdit`'s persisted state so that the
    /// next time it's drawn, the visible cursor jumps to the end of
    /// whatever was just inserted -- the same `set_char_range`-then-`store`
    /// pattern shown in egui's own `TextEditState` doc example.
    fn set_cursor(&self, ctx: &egui::Context, char_index: usize) {
        let id = self.source_edit_id();
        let mut state = TextEditState::load(ctx, id).unwrap_or_default();
        state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(char_index))));
        state.store(ctx, id);
    }

    fn tree_pane(&mut self, ui: &mut egui::Ui, app: &AppContext, for_image_picker: bool) {
        egui::ScrollArea::vertical()
            .id_salt(("writer_tree", self.instance_id(), for_image_picker))
            .show(ui, |ui| {
                self.tree_node(ui, app, "/".to_string(), 0, for_image_picker);
            });
    }

    fn tree_node(&mut self, ui: &mut egui::Ui, app: &AppContext, path: String, depth: usize, for_image_picker: bool) {
        if depth > 8 {
            return;
        }
        let Ok(entries) = app.vfs.list_entries(&path) else { return };
        for entry in entries {
            let child_path = if path.ends_with('/') { format!("{path}{}", entry.name) } else { format!("{path}/{}", entry.name) };
            let lower = entry.name.to_ascii_lowercase();
            let is_image = ["png", "jpg", "jpeg", "gif", "webp", "ico"].iter().any(|ext| lower.ends_with(&format!(".{ext}")));
            let is_wrr = lower.ends_with(".wrr");
            let relevant = if for_image_picker { is_image } else { is_wrr };
            ui.horizontal(|ui| {
                ui.add_space(depth as f32 * 12.0);
                if entry.is_dir {
                    ui.label(format!("\u{1f4c1} {}", entry.name));
                } else if relevant {
                    let is_open = !for_image_picker && self.path.as_deref() == Some(child_path.as_str());
                    let icon = if for_image_picker { "\u{1f5bc}" } else { "\u{1f4dd}" };
                    if ui.selectable_label(is_open, format!("{icon} {}", entry.name)).clicked() {
                        if for_image_picker {
                            let real_path = app.vfs.real_path(&child_path);
                            self.insert_at_cursor(ui.ctx(), &format!("![]({})", file_uri(&real_path)));
                            self.image_picked = true;
                        } else {
                            self.request(app, NextAction::Open(child_path.clone()));
                        }
                    }
                } else {
                    ui.weak(format!("\u{1f4c4} {}", entry.name));
                }
            });
            if entry.is_dir {
                self.tree_node(ui, app, child_path, depth + 1, for_image_picker);
            }
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, app: &AppContext) {
        ui.horizontal(|ui| {
            if ui.button(if self.show_tree { "\u{25c0} Files" } else { "\u{25b6} Files" }).clicked() {
                self.show_tree = !self.show_tree;
            }
            ui.separator();
            if ui.button("New").clicked() {
                self.request(app, NextAction::New);
            }
            if ui.button("Save").clicked() {
                self.save(app);
            }
            if ui.button("Save As...").clicked() {
                self.pending = Some(Pending::SaveAs { path: self.path.clone().unwrap_or_else(|| "/untitled.wrr".to_string()) });
            }
            if ui.button("Export as HTML...").clicked() {
                self.pending = Some(Pending::ExportHtml { path: "/sites/mysite/index.html".to_string() });
            }
        });
        ui.horizontal(|ui| {
            let ctx = ui.ctx().clone();
            if ui.button("B").on_hover_text("Bold").clicked() {
                self.wrap_selection(&ctx, "**", "**");
            }
            if ui.button("I").on_hover_text("Italic").clicked() {
                self.wrap_selection(&ctx, "*", "*");
            }
            if ui.button("</>").on_hover_text("Code").clicked() {
                self.wrap_selection(&ctx, "`", "`");
            }
            ui.separator();
            if ui.button("H1").clicked() {
                self.prefix_line(&ctx, "# ");
            }
            if ui.button("H2").clicked() {
                self.prefix_line(&ctx, "## ");
            }
            if ui.button("H3").clicked() {
                self.prefix_line(&ctx, "### ");
            }
            ui.separator();
            if ui.button("\u{2022} List").on_hover_text("Bullet list").clicked() {
                self.prefix_line(&ctx, "- ");
            }
            if ui.button("1. List").on_hover_text("Numbered list").clicked() {
                self.prefix_line(&ctx, "1. ");
            }
            if ui.button("\u{275d}\u{275e}").on_hover_text("Quote").clicked() {
                self.prefix_line(&ctx, "> ");
            }
            ui.separator();
            if ui.button("Link").clicked() {
                self.insert_at_cursor(&ctx, "[text](url)");
            }
            if ui.button("Image").clicked() {
                self.pending = Some(Pending::InsertImage);
            }
            ui.separator();
            ui.selectable_value(&mut self.view_mode, ViewMode::Split, "Split");
            ui.selectable_value(&mut self.view_mode, ViewMode::SourceOnly, "Source");
            ui.selectable_value(&mut self.view_mode, ViewMode::PreviewOnly, "Preview");
        });
        if let Some(status) = &self.status {
            ui.weak(status);
        }
        if let Some(error) = &self.error {
            ui.colored_label(theme::ERROR_MAROON, error);
        }
    }

    fn source_pane(&mut self, ui: &mut egui::Ui) {
        let id = self.source_edit_id();
        egui::ScrollArea::vertical().id_salt(("writer_source_scroll", self.instance_id())).show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut self.source)
                    .id(id)
                    .desired_width(f32::INFINITY)
                    .desired_rows(24)
                    .font(egui::TextStyle::Monospace),
            );
        });
    }

    fn preview_pane(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().id_salt(("writer_preview_scroll", self.instance_id())).show(ui, |ui| {
            egui_commonmark::CommonMarkViewer::new().show(ui, &mut self.preview_cache, &self.source);
        });
    }

    fn content(&mut self, ui: &mut egui::Ui) {
        match self.view_mode {
            ViewMode::SourceOnly => self.source_pane(ui),
            ViewMode::PreviewOnly => self.preview_pane(ui),
            ViewMode::Split => {
                ui.columns(2, |columns| {
                    self.source_pane(&mut columns[0]);
                    self.preview_pane(&mut columns[1]);
                });
            }
        }
    }

    fn export_html(&mut self, app: &AppContext, target_path: String) {
        let Some((dir, _filename)) = split_dir_and_name(&target_path) else {
            self.error = Some(format!("invalid export path: {target_path}"));
            return;
        };

        let mut image_copies: Vec<(String, String)> = Vec::new();
        let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
        let parser = Parser::new_ext(&self.source, options);
        let rewritten = parser.map(|event| rewrite_image_event(event, app.vfs, &mut image_copies));

        let mut body = String::new();
        html::push_html(&mut body, rewritten);

        let title = filename_stem(&target_path);
        let document = format!(
            "<!doctype html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n<title>{title}</title>\n<link rel=\"stylesheet\" href=\"style.css\">\n</head>\n<body>\n{body}</body>\n</html>\n"
        );

        if let Err(err) = app.vfs.write_string(&target_path, &document) {
            self.error = Some(format!("error writing {target_path}: {err:#}"));
            return;
        }
        let css_path = format!("{dir}/style.css");
        if let Err(err) = app.vfs.write_string(&css_path, DEFAULT_STYLE_CSS) {
            self.error = Some(format!("error writing {css_path}: {err:#}"));
            return;
        }
        for (src, name) in &image_copies {
            let dest = format!("{dir}/{name}");
            if let Err(err) = app.vfs.cp(src, &dest) {
                self.error = Some(format!("error copying image {src}: {err:#}"));
                return;
            }
        }

        self.error = None;
        self.status = Some(format!(
            "Exported to {target_path}. Run `publish <domain.rym>` from Terminal in {dir} to publish it."
        ));
    }

    fn pending_dialog(&mut self, ctx: &egui::Context, app: &AppContext) {
        let Some(pending) = self.pending.take() else { return };
        let instance_id = self.instance_id();
        let mut keep_open = true;

        match pending {
            Pending::SaveAs { mut path } => {
                egui::Window::new("Save As")
                    .id(egui::Id::new(("writer_save_as", instance_id)))
                    .collapsible(false)
                    .resizable(false)
                    .show(ctx, |ui| {
                        ui.label("Path in your vfs:");
                        ui.text_edit_singleline(&mut path);
                        ui.horizontal(|ui| {
                            if ui.button("OK").clicked() && !path.trim().is_empty() {
                                self.save_to(app, path.trim().to_string());
                                keep_open = false;
                            }
                            if ui.button("Cancel").clicked() {
                                keep_open = false;
                            }
                        });
                    });
                if keep_open {
                    self.pending = Some(Pending::SaveAs { path });
                }
            }
            Pending::ExportHtml { mut path } => {
                egui::Window::new("Export as HTML")
                    .id(egui::Id::new(("writer_export_html", instance_id)))
                    .collapsible(false)
                    .resizable(false)
                    .show(ctx, |ui| {
                        ui.label("Target .html path in your vfs (a style.css and any images go alongside it):");
                        ui.text_edit_singleline(&mut path);
                        ui.horizontal(|ui| {
                            if ui.button("OK").clicked() && !path.trim().is_empty() {
                                self.export_html(app, path.trim().to_string());
                                keep_open = false;
                            }
                            if ui.button("Cancel").clicked() {
                                keep_open = false;
                            }
                        });
                    });
                if keep_open {
                    self.pending = Some(Pending::ExportHtml { path });
                }
            }
            Pending::InsertImage => {
                self.image_picked = false;
                egui::Window::new("Insert Image")
                    .id(egui::Id::new(("writer_insert_image", instance_id)))
                    .collapsible(false)
                    .default_size([320.0, 360.0])
                    .show(ctx, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            self.tree_pane(ui, app, true);
                        });
                        if ui.button("Cancel").clicked() {
                            keep_open = false;
                        }
                    });
                if self.image_picked {
                    keep_open = false;
                }
                if keep_open {
                    self.pending = Some(Pending::InsertImage);
                }
            }
            Pending::ConfirmDiscard { then } => {
                let mut discard = false;
                egui::Window::new("Discard unsaved changes?")
                    .id(egui::Id::new(("writer_confirm_discard", instance_id)))
                    .collapsible(false)
                    .resizable(false)
                    .show(ctx, |ui| {
                        ui.label("This document has unsaved changes.");
                        ui.horizontal(|ui| {
                            if ui.button("Discard").clicked() {
                                discard = true;
                                keep_open = false;
                            }
                            if ui.button("Cancel").clicked() {
                                keep_open = false;
                            }
                        });
                    });
                if discard {
                    self.perform(app, then);
                } else if keep_open {
                    self.pending = Some(Pending::ConfirmDiscard { then });
                }
            }
        }
    }
}

impl RymApp for WriterApp {
    fn title(&self) -> String {
        "Writer".to_string()
    }

    fn show(&mut self, ctx: &egui::Context, app: &AppContext, _launch: &mut Vec<Box<dyn RymApp>>) -> bool {
        let mut open = true;
        theme::DecoratedWindow::new(egui::Id::new(("writer_window", self.instance_id())), self.title())
            .icon("\u{1f4c4}")
            .default_size([900.0, 600.0])
            .show(ctx, &mut open, |ui| {
                self.toolbar(ui, app);
                ui.separator();
                if self.show_tree {
                    egui::Panel::left(egui::Id::new(("writer_tree_panel", self.instance_id())))
                        .default_size(180.0)
                        .show(ui, |ui| {
                            self.tree_pane(ui, app, false);
                        });
                }
                self.content(ui);
            });
        self.pending_dialog(ctx, app);
        open
    }
}

fn file_uri(path: &std::path::Path) -> String {
    format!("file://{}", path.display())
}

fn split_dir_and_name(path: &str) -> Option<(String, String)> {
    let trimmed = path.trim_end_matches('/');
    let idx = trimmed.rfind('/')?;
    let dir = if idx == 0 { "/".to_string() } else { trimmed[..idx].to_string() };
    let name = trimmed[idx + 1..].to_string();
    if name.is_empty() { None } else { Some((dir, name)) }
}

fn filename_stem(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.strip_suffix(".html").unwrap_or(name).to_string()
}

/// Rewrites `file://` image references emitted while parsing the source
/// markdown into bare filenames (portable once published), queuing the
/// original vfs source path + chosen filename for a real file copy
/// alongside the exported HTML. Every other event passes through
/// unchanged. Operating on the `Event` stream (not a second text pass)
/// keeps the export parse identical to the preview parse -- no risk of
/// the two drifting apart.
fn rewrite_image_event<'a>(event: Event<'a>, vfs: &VirtualComputer, image_copies: &mut Vec<(String, String)>) -> Event<'a> {
    let Event::Start(Tag::Image { link_type, dest_url, title, id }) = event else {
        return event;
    };
    let Some(real_path) = dest_url.strip_prefix("file://") else {
        return Event::Start(Tag::Image { link_type, dest_url, title, id });
    };
    let real_path = std::path::PathBuf::from(real_path);
    let Some(vfs_path) = vfs.vfs_path_for(&real_path) else {
        return Event::Start(Tag::Image { link_type, dest_url, title, id });
    };
    let filename = real_path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "image".to_string());
    image_copies.push((vfs_path, filename.clone()));
    Event::Start(Tag::Image { link_type, dest_url: filename.into(), title, id })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_range(ctx: &egui::Context, id: egui::Id, start: usize, end: usize) {
        let mut state = TextEditState::load(ctx, id).unwrap_or_default();
        state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(start), CCursor::new(end))));
        state.store(ctx, id);
    }

    #[test]
    fn wrap_selection_wraps_the_selected_text() {
        let ctx = egui::Context::default();
        let mut app = WriterApp::new();
        app.source = "hello world".to_string();
        set_range(&ctx, app.source_edit_id(), 0, 5); // "hello"
        app.wrap_selection(&ctx, "**", "**");
        assert_eq!(app.source, "**hello** world");
    }

    #[test]
    fn wrap_selection_with_no_selection_inserts_an_empty_wrapper_at_the_cursor() {
        let ctx = egui::Context::default();
        let mut app = WriterApp::new();
        app.source = "hi".to_string();
        set_range(&ctx, app.source_edit_id(), 2, 2); // cursor at the end, nothing selected
        app.wrap_selection(&ctx, "*", "*");
        assert_eq!(app.source, "hi**");
        // The cursor should land between the two inserted markers, not after them.
        let range = app.cursor_range(&ctx).expect("cursor range was stored");
        assert_eq!(range.primary.index.0, 3);
    }

    #[test]
    fn wrap_selection_uses_character_offsets_not_byte_offsets() {
        // "héllo" is 5 characters but 6 UTF-8 bytes (é is 2 bytes) -- if
        // `wrap_selection` sliced by byte offset instead of char offset,
        // this would either panic (splitting a multi-byte char) or select
        // the wrong text.
        let ctx = egui::Context::default();
        let mut app = WriterApp::new();
        app.source = "héllo wörld".to_string();
        set_range(&ctx, app.source_edit_id(), 0, 5); // the 5 characters of "héllo"
        app.wrap_selection(&ctx, "**", "**");
        assert_eq!(app.source, "**héllo** wörld");
    }

    #[test]
    fn prefix_line_inserts_at_the_start_of_the_current_line_not_the_document() {
        let ctx = egui::Context::default();
        let mut app = WriterApp::new();
        app.source = "first line\nsecond line".to_string();
        let cursor_pos = "first line\nsec".chars().count(); // partway into the second line
        set_range(&ctx, app.source_edit_id(), cursor_pos, cursor_pos);
        app.prefix_line(&ctx, "# ");
        assert_eq!(app.source, "first line\n# second line");
    }

    #[test]
    fn insert_at_cursor_places_text_at_the_cursor_and_moves_past_it() {
        let ctx = egui::Context::default();
        let mut app = WriterApp::new();
        app.source = "before after".to_string();
        set_range(&ctx, app.source_edit_id(), 7, 7); // right before "after"
        app.insert_at_cursor(&ctx, "middle ");
        assert_eq!(app.source, "before middle after");
    }

    fn scratch_vfs() -> VirtualComputer {
        let dir = std::env::temp_dir().join(format!("rymwin-writer-test-{}-{}", std::process::id(), rand_suffix()));
        let _ = std::fs::remove_dir_all(&dir);
        VirtualComputer::open(dir).unwrap()
    }

    fn rand_suffix() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64
    }

    #[test]
    fn rewrite_image_event_rewrites_file_uri_to_a_relative_filename_and_queues_a_copy() {
        let vfs = scratch_vfs();
        vfs.write_string("/images/cat.png", "fake-png-bytes").unwrap();
        let real_path = vfs.real_path("/images/cat.png");

        let markdown = format!("![a cat]({})", file_uri(&real_path));
        let mut copies = Vec::new();
        let events: Vec<Event> = Parser::new(&markdown).map(|event| rewrite_image_event(event, &vfs, &mut copies)).collect();

        let mut rendered = String::new();
        html::push_html(&mut rendered, events.into_iter());

        assert_eq!(copies, vec![("/images/cat.png".to_string(), "cat.png".to_string())]);
        assert!(rendered.contains("src=\"cat.png\""), "expected a rewritten relative filename in: {rendered}");
        assert!(!rendered.contains("file://"), "a file:// uri must never leak into exported html: {rendered}");

        let _ = std::fs::remove_dir_all(&vfs.root);
    }

    #[test]
    fn rewrite_image_event_leaves_non_file_uris_untouched() {
        let vfs = scratch_vfs();
        let markdown = "![remote](https://example.com/pic.png)";
        let mut copies = Vec::new();
        let events: Vec<Event> = Parser::new(markdown).map(|event| rewrite_image_event(event, &vfs, &mut copies)).collect();

        let mut rendered = String::new();
        html::push_html(&mut rendered, events.into_iter());

        assert!(copies.is_empty());
        assert!(rendered.contains("src=\"https://example.com/pic.png\""));

        let _ = std::fs::remove_dir_all(&vfs.root);
    }
}
