use crate::app::{AppContext, RymApp};
use crate::theme;
use eframe::egui;

const MAX_UNDO: usize = 20;
const DEFAULT_WIDTH: u32 = 800;
const DEFAULT_HEIGHT: u32 = 600;
const MIN_BRUSH: u32 = 1;
const MAX_BRUSH: u32 = 64;

#[derive(PartialEq, Clone, Copy)]
enum Tool {
    Pencil,
    Eraser,
    Fill,
}

struct Canvas {
    width: u32,
    height: u32,
    pixels: Vec<u8>, // RGBA8, width*height*4 bytes
    texture: egui::TextureHandle,
    dirty: bool,
    path: Option<String>,
}

enum Pending {
    NewCanvas { width: String, height: String },
    SaveAs { name: String },
}

/// The eighth desktop app: a simple pixel editor for PNG images. Unlike
/// PDF Reader, this needs no native dependency and no background worker
/// -- PNG decode/encode is fast, pure-Rust `image` crate work, and
/// painting is just mutating an in-memory RGBA buffer each frame, the
/// same "do it inline on the GUI thread" approach `RymcodeApp`/
/// `FileManagerApp` already use for their own file I/O.
pub struct PaintApp {
    canvas: Option<Canvas>,
    tool: Tool,
    brush_size: u32,
    color: egui::Color32,
    undo_stack: Vec<Vec<u8>>,
    stroke_active: bool,
    pending: Option<Pending>,
    status: Option<String>,
    error: Option<String>,
    show_tree: bool,
}

impl PaintApp {
    pub fn new() -> Self {
        Self {
            canvas: None,
            tool: Tool::Pencil,
            brush_size: 4,
            color: egui::Color32::BLACK,
            undo_stack: Vec::new(),
            stroke_active: false,
            pending: None,
            status: None,
            error: None,
            show_tree: true,
        }
    }

    fn instance_id(&self) -> usize {
        self as *const Self as usize
    }

    fn new_canvas(&mut self, ctx: &egui::Context, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        let pixels = vec![255u8; (width * height * 4) as usize];
        let texture = upload_texture(ctx, self.instance_id(), width, height, &pixels);
        self.canvas = Some(Canvas { width, height, pixels, texture, dirty: false, path: None });
        self.undo_stack.clear();
        self.stroke_active = false;
        self.status = None;
        self.error = None;
    }

    fn open(&mut self, ctx: &egui::Context, app: &AppContext, path: String) {
        let real_path = app.vfs.real_path(&path);
        match image::open(&real_path) {
            Ok(image) => {
                let rgba = image.to_rgba8();
                let (width, height) = (rgba.width(), rgba.height());
                let pixels = rgba.into_raw();
                let texture = upload_texture(ctx, self.instance_id(), width, height, &pixels);
                self.canvas = Some(Canvas { width, height, pixels, texture, dirty: false, path: Some(path) });
                self.undo_stack.clear();
                self.stroke_active = false;
                self.status = None;
                self.error = None;
            }
            Err(err) => self.error = Some(format!("error opening {}: {err}", real_path.display())),
        }
    }

    fn save_to(&mut self, app: &AppContext, path: String) {
        let Some(canvas) = &mut self.canvas else { return };
        let Some(buffer) = image::RgbaImage::from_raw(canvas.width, canvas.height, canvas.pixels.clone()) else {
            self.error = Some("could not build image from canvas buffer".to_string());
            return;
        };
        let real_path = app.vfs.real_path(&path);
        match buffer.save(&real_path) {
            Ok(()) => {
                canvas.path = Some(path.clone());
                self.status = Some(format!("saved {path}"));
                self.error = None;
            }
            Err(err) => self.error = Some(format!("error saving {}: {err}", real_path.display())),
        }
    }

    fn save(&mut self, app: &AppContext) {
        let Some(existing) = self.canvas.as_ref().and_then(|c| c.path.clone()) else {
            self.pending = Some(Pending::SaveAs { name: "untitled.png".to_string() });
            return;
        };
        self.save_to(app, existing);
    }

    fn push_undo_if_new_stroke(&mut self) {
        if self.stroke_active {
            return;
        }
        self.stroke_active = true;
        let Some(canvas) = &self.canvas else { return };
        self.undo_stack.push(canvas.pixels.clone());
        if self.undo_stack.len() > MAX_UNDO {
            self.undo_stack.remove(0);
        }
    }

    fn undo(&mut self) {
        let Some(previous) = self.undo_stack.pop() else { return };
        let Some(canvas) = &mut self.canvas else { return };
        canvas.pixels = previous;
        canvas.dirty = true;
    }

    fn paint_at(&mut self, x: i32, y: i32) {
        let Some(canvas) = &mut self.canvas else { return };
        let color = if self.tool == Tool::Eraser { [0, 0, 0, 0] } else { self.color.to_srgba_unmultiplied() };
        let half = (self.brush_size / 2) as i32;
        for dy in -half..=half {
            for dx in -half..=half {
                let px = x + dx;
                let py = y + dy;
                if px < 0 || py < 0 || px as u32 >= canvas.width || py as u32 >= canvas.height {
                    continue;
                }
                set_pixel(&mut canvas.pixels, canvas.width, px as u32, py as u32, color);
            }
        }
        canvas.dirty = true;
    }

    fn fill_at(&mut self, x: i32, y: i32) {
        let Some(canvas) = &mut self.canvas else { return };
        if x < 0 || y < 0 || x as u32 >= canvas.width || y as u32 >= canvas.height {
            return;
        }
        let target = get_pixel(&canvas.pixels, canvas.width, x as u32, y as u32);
        let replacement = self.color.to_srgba_unmultiplied();
        if target == replacement {
            return;
        }
        flood_fill(&mut canvas.pixels, canvas.width, canvas.height, x as u32, y as u32, target, replacement);
        canvas.dirty = true;
    }

    fn tree_pane(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, app: &AppContext) {
        egui::ScrollArea::vertical()
            .id_salt(("paint_tree", self.instance_id()))
            .show(ui, |ui| {
                self.tree_node(ui, ctx, app, "/".to_string(), 0);
            });
    }

    fn tree_node(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, app: &AppContext, path: String, depth: usize) {
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
                    let is_open = self.canvas.as_ref().and_then(|c| c.path.as_deref()) == Some(child_path.as_str());
                    let icon = if entry.name.to_ascii_lowercase().ends_with(".png") { "\u{1f5bc}" } else { "\u{1f4c4}" };
                    if ui.selectable_label(is_open, format!("{icon} {}", entry.name)).clicked() {
                        self.open(ctx, app, child_path.clone());
                    }
                }
            });
            if entry.is_dir {
                self.tree_node(ui, ctx, app, child_path, depth + 1);
            }
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, app: &AppContext) {
        ui.horizontal(|ui| {
            if ui.button(if self.show_tree { "\u{25c0} Files" } else { "\u{25b6} Files" }).clicked() {
                self.show_tree = !self.show_tree;
            }
            ui.separator();
            if ui.button("New").clicked() {
                self.pending = Some(Pending::NewCanvas { width: DEFAULT_WIDTH.to_string(), height: DEFAULT_HEIGHT.to_string() });
            }
            let has_canvas = self.canvas.is_some();
            if ui.add_enabled(has_canvas, egui::Button::new("Save")).clicked() {
                self.save(app);
            }
            if ui.add_enabled(has_canvas, egui::Button::new("Save As...")).clicked() {
                self.pending = Some(Pending::SaveAs { name: "untitled.png".to_string() });
            }
            ui.separator();
            ui.selectable_value(&mut self.tool, Tool::Pencil, "\u{270f} Pencil");
            ui.selectable_value(&mut self.tool, Tool::Eraser, "\u{1f9fd} Eraser");
            ui.selectable_value(&mut self.tool, Tool::Fill, "\u{1faa3} Fill");
            ui.separator();
            ui.label("Brush:");
            ui.add(egui::Slider::new(&mut self.brush_size, MIN_BRUSH..=MAX_BRUSH));
            ui.color_edit_button_srgba(&mut self.color);
            ui.separator();
            if ui.add_enabled(!self.undo_stack.is_empty(), egui::Button::new("\u{21b6} Undo")).clicked() {
                self.undo();
            }
        });
        if let Some(status) = &self.status {
            ui.weak(status);
        }
        if let Some(error) = &self.error {
            ui.colored_label(theme::ERROR_MAROON, error);
        }
        let _ = ctx;
    }

    fn canvas_area(&mut self, ui: &mut egui::Ui) {
        let Some(canvas) = &self.canvas else {
            ui.weak("Open a .png from the file list, or click New to start a blank canvas.");
            return;
        };
        let texture_id = canvas.texture.id();
        let size = canvas.texture.size_vec2();
        let mut paint_targets: Vec<(i32, i32)> = Vec::new();
        let mut fill_target: Option<(i32, i32)> = None;
        let mut stroke_ended = false;

        egui::ScrollArea::both().id_salt(("paint_canvas_scroll", self.instance_id())).show(ui, |ui| {
            let response = ui.add(egui::Image::new((texture_id, size)).sense(egui::Sense::click_and_drag()));
            if self.tool == Tool::Fill {
                if response.clicked()
                    && let Some(pos) = response.interact_pointer_pos()
                {
                    let local = pos - response.rect.min;
                    fill_target = Some((local.x as i32, local.y as i32));
                }
            } else if response.dragged() || response.drag_stopped() {
                if let Some(pos) = response.interact_pointer_pos() {
                    let local = pos - response.rect.min;
                    paint_targets.push((local.x as i32, local.y as i32));
                }
                if response.drag_stopped() {
                    stroke_ended = true;
                }
            }
        });

        if let Some((x, y)) = fill_target {
            self.push_undo_if_new_stroke();
            self.fill_at(x, y);
            self.stroke_active = false;
        }
        if !paint_targets.is_empty() {
            self.push_undo_if_new_stroke();
            for (x, y) in paint_targets {
                self.paint_at(x, y);
            }
        }
        if stroke_ended {
            self.stroke_active = false;
        }
    }

    fn pending_dialog(&mut self, ctx: &egui::Context, app: &AppContext) {
        let Some(pending) = self.pending.take() else { return };
        let instance_id = self.instance_id();
        let mut keep_open = true;

        match pending {
            Pending::NewCanvas { mut width, mut height } => {
                egui::Window::new("New Canvas")
                    .id(egui::Id::new(("paint_new_canvas", instance_id)))
                    .collapsible(false)
                    .resizable(false)
                    .show(ctx, |ui| {
                        ui.horizontal(|ui| {
                            ui.label("Width:");
                            ui.text_edit_singleline(&mut width);
                            ui.label("Height:");
                            ui.text_edit_singleline(&mut height);
                        });
                        ui.horizontal(|ui| {
                            let parsed = width.trim().parse::<u32>().ok().zip(height.trim().parse::<u32>().ok());
                            if ui.add_enabled(parsed.is_some(), egui::Button::new("OK")).clicked()
                                && let Some((w, h)) = parsed
                            {
                                self.new_canvas(ctx, w, h);
                                keep_open = false;
                            }
                            if ui.button("Cancel").clicked() {
                                keep_open = false;
                            }
                        });
                    });
                if keep_open {
                    self.pending = Some(Pending::NewCanvas { width, height });
                }
            }
            Pending::SaveAs { mut name } => {
                egui::Window::new("Save As")
                    .id(egui::Id::new(("paint_save_as", instance_id)))
                    .collapsible(false)
                    .resizable(false)
                    .show(ctx, |ui| {
                        ui.label("Path in your vfs:");
                        ui.text_edit_singleline(&mut name);
                        ui.horizontal(|ui| {
                            if ui.button("OK").clicked() && !name.trim().is_empty() {
                                self.save_to(app, name.trim().to_string());
                                keep_open = false;
                            }
                            if ui.button("Cancel").clicked() {
                                keep_open = false;
                            }
                        });
                    });
                if keep_open {
                    self.pending = Some(Pending::SaveAs { name });
                }
            }
        }
    }
}

impl RymApp for PaintApp {
    fn title(&self) -> String {
        "Paint".to_string()
    }

    fn show(&mut self, ctx: &egui::Context, app: &AppContext, _launch: &mut Vec<Box<dyn RymApp>>) -> bool {
        let mut open = true;
        theme::DecoratedWindow::new(egui::Id::new(("paint_window", self.instance_id())), self.title())
            .icon("\u{1f3a8}")
            .default_size([760.0, 560.0])
            .show(ctx, &mut open, |ui| {
                self.toolbar(ui, ctx, app);
                ui.separator();
                if self.show_tree {
                    egui::Panel::left(egui::Id::new(("paint_tree_panel", self.instance_id())))
                        .default_size(180.0)
                        .show(ui, |ui| {
                            self.tree_pane(ui, ctx, app);
                        });
                }
                self.canvas_area(ui);
            });

        // Re-upload the texture only on frames the pixel buffer actually
        // changed, not unconditionally every frame -- the same
        // dirty-flag idea `pdfreaderapp.rs` uses, just triggered locally
        // instead of by an incoming channel event.
        let instance_id = self.instance_id();
        if let Some(canvas) = &mut self.canvas
            && canvas.dirty
        {
            canvas.texture = upload_texture(ctx, instance_id, canvas.width, canvas.height, &canvas.pixels);
            canvas.dirty = false;
        }

        self.pending_dialog(ctx, app);
        open
    }
}

fn upload_texture(ctx: &egui::Context, instance_id: usize, width: u32, height: u32, pixels: &[u8]) -> egui::TextureHandle {
    let image = egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], pixels);
    ctx.load_texture(format!("paint_canvas_{instance_id}"), image, egui::TextureOptions::NEAREST)
}

fn pixel_index(width: u32, x: u32, y: u32) -> usize {
    ((y * width + x) * 4) as usize
}

fn get_pixel(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let i = pixel_index(width, x, y);
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

fn set_pixel(pixels: &mut [u8], width: u32, x: u32, y: u32, color: [u8; 4]) {
    let i = pixel_index(width, x, y);
    pixels[i..i + 4].copy_from_slice(&color);
}

/// Iterative (not recursive, to avoid a stack overflow on a large
/// same-color region), 4-connected flood fill from `(start_x, start_y)`,
/// replacing every reachable pixel that exactly matches `target` with
/// `replacement`. No tolerance/anti-aliasing -- exact color match only,
/// matching this app's overall "simple editor" scope.
fn flood_fill(pixels: &mut [u8], width: u32, height: u32, start_x: u32, start_y: u32, target: [u8; 4], replacement: [u8; 4]) {
    let mut stack = vec![(start_x, start_y)];
    while let Some((x, y)) = stack.pop() {
        if get_pixel(pixels, width, x, y) != target {
            continue;
        }
        set_pixel(pixels, width, x, y, replacement);
        if x > 0 {
            stack.push((x - 1, y));
        }
        if x + 1 < width {
            stack.push((x + 1, y));
        }
        if y > 0 {
            stack.push((x, y - 1));
        }
        if y + 1 < height {
            stack.push((x, y + 1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_checkerboard(width: u32, height: u32) -> Vec<u8> {
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        for y in 0..height {
            for x in 0..width {
                let color = if (x + y) % 2 == 0 { [255, 255, 255, 255] } else { [0, 0, 0, 255] };
                set_pixel(&mut pixels, width, x, y, color);
            }
        }
        pixels
    }

    #[test]
    fn flood_fill_only_changes_connected_same_color_pixels() {
        // A 4x4 checkerboard has no two same-colored pixels adjacent to
        // each other, so filling from (0,0) should only ever change that
        // single pixel.
        let mut pixels = make_checkerboard(4, 4);
        let target = get_pixel(&pixels, 4, 0, 0);
        flood_fill(&mut pixels, 4, 4, 0, 0, target, [255, 0, 0, 255]);
        assert_eq!(get_pixel(&pixels, 4, 0, 0), [255, 0, 0, 255]);
        // Every other white pixel on the board should be untouched.
        for y in 0..4u32 {
            for x in 0..4u32 {
                if (x, y) == (0, 0) {
                    continue;
                }
                assert_ne!(get_pixel(&pixels, 4, x, y), [255, 0, 0, 255], "pixel ({x},{y}) should not have been touched");
            }
        }
    }

    #[test]
    fn flood_fill_fills_a_whole_solid_region() {
        // A solid 3x3 block should fill entirely from any starting pixel
        // inside it.
        let mut pixels = vec![255u8; 3 * 3 * 4];
        flood_fill(&mut pixels, 3, 3, 1, 1, [255, 255, 255, 255], [0, 128, 0, 255]);
        for y in 0..3u32 {
            for x in 0..3u32 {
                assert_eq!(get_pixel(&pixels, 3, x, y), [0, 128, 0, 255]);
            }
        }
    }

    #[test]
    fn flood_fill_does_not_cross_a_different_color_border() {
        // A 5x1 strip: white, white, black, white, white. Filling from
        // the left should only reach the first two pixels.
        let width = 5;
        let mut pixels = vec![0u8; (width * 4) as usize];
        for x in 0..width {
            let color = if x == 2 { [0, 0, 0, 255] } else { [255, 255, 255, 255] };
            set_pixel(&mut pixels, width, x, 0, color);
        }
        flood_fill(&mut pixels, width, 1, 0, 0, [255, 255, 255, 255], [0, 0, 255, 255]);
        assert_eq!(get_pixel(&pixels, width, 0, 0), [0, 0, 255, 255]);
        assert_eq!(get_pixel(&pixels, width, 1, 0), [0, 0, 255, 255]);
        assert_eq!(get_pixel(&pixels, width, 2, 0), [0, 0, 0, 255]); // untouched border
        assert_eq!(get_pixel(&pixels, width, 3, 0), [255, 255, 255, 255]); // unreached
        assert_eq!(get_pixel(&pixels, width, 4, 0), [255, 255, 255, 255]); // unreached
    }

    /// Exercises the exact same `image` crate calls `open`/`save_to` use
    /// (`image::open(..).to_rgba8()` and `RgbaImage::from_raw(..).save(..)`),
    /// self-contained -- no external fixture, unlike PDF Reader's native
    /// library, since the test PNG is generated by the same crate it's
    /// verifying, at a scratch path cleaned up afterward.
    #[test]
    fn png_round_trips_through_save_and_open_byte_for_byte() {
        let path = std::env::temp_dir().join(format!("rymwin-paint-test-{}.png", std::process::id()));
        let width = 4;
        let height = 4;
        let original = make_checkerboard(width, height);

        let buffer = image::RgbaImage::from_raw(width, height, original.clone()).expect("build image buffer");
        buffer.save(&path).expect("save png");

        let reopened = image::open(&path).expect("open png").to_rgba8();
        assert_eq!(reopened.width(), width);
        assert_eq!(reopened.height(), height);
        assert_eq!(reopened.into_raw(), original, "pixels must round-trip exactly through save+open");

        let _ = std::fs::remove_file(&path);
    }
}
