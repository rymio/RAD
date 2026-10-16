use eframe::egui;

// A curated retro palette (the classic VGA/Windows-3.1-era 16 colors), not
// a literal 8-bit indexed-color renderer -- there's no hardware left to
// target that mode on, so the "256 colors" brief becomes "constrain every
// UI element to draw from this fixed set" instead.
pub const DESKTOP_TEAL: egui::Color32 = egui::Color32::from_rgb(0, 128, 128);
pub const FACE_GRAY: egui::Color32 = egui::Color32::from_rgb(192, 192, 192);
pub const SHADOW_GRAY: egui::Color32 = egui::Color32::from_rgb(128, 128, 128);
pub const HIGHLIGHT_WHITE: egui::Color32 = egui::Color32::from_rgb(255, 255, 255);
pub const TITLE_NAVY: egui::Color32 = egui::Color32::from_rgb(0, 0, 128);
pub const TEXT_BLACK: egui::Color32 = egui::Color32::from_rgb(0, 0, 0);
pub const WINDOW_WHITE: egui::Color32 = egui::Color32::from_rgb(255, 255, 255);
pub const ERROR_MAROON: egui::Color32 = egui::Color32::from_rgb(128, 0, 0);

pub fn retro_visuals() -> egui::Visuals {
    let mut visuals = egui::Visuals::light();
    visuals.window_fill = FACE_GRAY;
    visuals.panel_fill = FACE_GRAY;
    visuals.override_text_color = Some(TEXT_BLACK);
    visuals.window_corner_radius = egui::CornerRadius::ZERO;
    visuals.menu_corner_radius = egui::CornerRadius::ZERO;

    for widgets in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widgets.corner_radius = egui::CornerRadius::ZERO;
        widgets.bg_fill = FACE_GRAY;
        widgets.weak_bg_fill = FACE_GRAY;
        widgets.fg_stroke = egui::Stroke::new(1.0, TEXT_BLACK);
    }
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, SHADOW_GRAY);
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, TITLE_NAVY);
    visuals.widgets.active.bg_fill = egui::Color32::from_rgb(220, 220, 220);
    visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, TEXT_BLACK);
    visuals.selection.bg_fill = TITLE_NAVY;
    visuals.selection.stroke = egui::Stroke::new(1.0, HIGHLIGHT_WHITE);
    visuals.extreme_bg_color = WINDOW_WHITE;

    visuals
}

/// A hand-drawn beveled rectangle (light top/left, dark bottom/right) --
/// the one visual signature that reads as "old Windows" more than any
/// single color choice does.
pub fn bevel_box(ui: &mut egui::Ui, rect: egui::Rect, raised: bool) {
    let (top_left, bottom_right) = if raised {
        (HIGHLIGHT_WHITE, SHADOW_GRAY)
    } else {
        (SHADOW_GRAY, HIGHLIGHT_WHITE)
    };
    let painter = ui.painter();
    painter.line_segment(
        [rect.left_top(), rect.right_top()],
        egui::Stroke::new(1.0, top_left),
    );
    painter.line_segment(
        [rect.left_top(), rect.left_bottom()],
        egui::Stroke::new(1.0, top_left),
    );
    painter.line_segment(
        [rect.right_top(), rect.right_bottom()],
        egui::Stroke::new(1.0, bottom_right),
    );
    painter.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        egui::Stroke::new(1.0, bottom_right),
    );
}

/// The taskbar's own fixed height (kept in sync with `taskbar.rs`'s
/// `exact_size` call) -- shared here so a maximized window's bottom edge
/// lands exactly above it instead of running underneath.
pub const TASKBAR_HEIGHT: f32 = 30.0;

/// Estimated height of the desktop's own top title bar (`main.rs`'s
/// `title_bar` panel). It isn't `exact_size`'d, so this is a measurement,
/// not a guarantee -- a maximized window may sit a few pixels off from
/// flush against it rather than pixel-perfect.
const DESKTOP_TOP_BAR_HEIGHT: f32 = 28.0;

const CHROME_TITLE_BAR_HEIGHT: f32 = 20.0;
const CHROME_BUTTON_SIZE: f32 = 18.0;
const CHROME_RESIZE_GRIP: f32 = 14.0;
const CHROME_MIN_SIZE: egui::Vec2 = egui::vec2(160.0, 100.0);

/// The usable desktop area a maximized window should fill: the full
/// viewport minus the top title bar and bottom taskbar strips.
fn desktop_rect(ctx: &egui::Context) -> egui::Rect {
    let mut rect = ctx.content_rect();
    rect.min.y += DESKTOP_TOP_BAR_HEIGHT;
    rect.max.y -= TASKBAR_HEIGHT;
    rect
}

#[derive(Clone, Copy, Debug)]
struct ChromeState {
    /// Geometry while neither minimized nor maximized -- kept up to date
    /// even while minimized/maximized (as the "restore to" point), since
    /// dragging/resizing are only ever applied to it, never to the
    /// minimized/maximized *displayed* rect itself.
    rect: egui::Rect,
    minimized: bool,
    maximized: bool,
}

/// A small title bar control button (close/minimize/maximize), drawn and
/// sensed entirely by hand with an explicit `id` -- not `egui::Button`,
/// whose id is derived from its label text and position rather than
/// anything the caller controls, which is fine for a normal one-off
/// button but makes automated tests unable to reliably find one of
/// several look-alike glyph buttons in a title bar (see `chrome_tests`).
fn chrome_button(ui: &mut egui::Ui, id: egui::Id, label: &str) -> egui::Response {
    let size = egui::vec2(CHROME_BUTTON_SIZE, CHROME_BUTTON_SIZE);
    let (_, rect) = ui.allocate_space(size);
    let response = ui.interact(rect, id, egui::Sense::click());
    let pressed = response.is_pointer_button_down_on();
    ui.painter().rect_filled(rect, 0.0, FACE_GRAY);
    bevel_box(ui, rect, !pressed);
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, label, egui::FontId::default(), TEXT_BLACK);
    response
}

/// A Windows 3.1-style "group window" decorator: a hand-drawn title bar
/// (control-menu icon, title, minimize/maximize buttons) and beveled
/// border, standing in for egui's own default `Window` chrome (a plain
/// label plus a collapse arrow and a close X, none of which reads as this
/// era of Windows). Because the native title bar is what egui normally
/// uses to drag/resize a `Window`, disabling it here means dragging,
/// resizing, minimizing, and maximizing all have to be hand-rolled too --
/// geometry lives in `ctx`'s per-frame temp storage, keyed off the
/// window's own `Id`, the same idiom `taskbar.rs` already uses for its
/// own transient UI state (so nothing needs to be threaded through every
/// app's own struct fields).
pub struct DecoratedWindow {
    id: egui::Id,
    title: String,
    icon: &'static str,
    default_size: egui::Vec2,
}

impl DecoratedWindow {
    pub fn new(id: egui::Id, title: impl Into<String>) -> Self {
        Self { id, title: title.into(), icon: "\u{25a3}", default_size: egui::vec2(360.0, 240.0) }
    }

    /// A short glyph (single char or emoji) shown in the title bar's
    /// control-menu box -- double-clicking it closes the window, the
    /// Windows 3.1 convention (this era's chrome has no dedicated close
    /// button, only minimize/maximize).
    pub fn icon(mut self, icon: &'static str) -> Self {
        self.icon = icon;
        self
    }

    pub fn default_size(mut self, size: impl Into<egui::Vec2>) -> Self {
        self.default_size = size.into();
        self
    }

    /// Draws the window and runs `add_contents` unless it's minimized
    /// (nothing below the title bar to draw into), mirroring
    /// `egui::Window::show` returning `None` while collapsed. Closing
    /// (the title bar's close button) sets `*open = false`; the caller
    /// drops the window next frame exactly as it already does for
    /// `egui::Window`'s own `.open(&mut open)`.
    pub fn show<R>(
        self,
        ctx: &egui::Context,
        open: &mut bool,
        add_contents: impl FnOnce(&mut egui::Ui) -> R,
    ) -> Option<R> {
        if !*open {
            return None;
        }

        let state_id = self.id.with("chrome_state");
        let desktop = desktop_rect(ctx);
        let mut state = ctx.data(|d| d.get_temp::<ChromeState>(state_id)).unwrap_or_else(|| {
            // New windows cascade down-and-right instead of stacking
            // exactly on top of each other -- jitter derived from the
            // window's own id, so it's stable across frames without a
            // separate "how many windows are already open" counter.
            let jitter = (self.id.value() % 8) as f32 * 22.0;
            let min_pos = desktop.min + egui::vec2(30.0 + jitter, 20.0 + jitter);
            ChromeState {
                rect: egui::Rect::from_min_size(min_pos, self.default_size),
                minimized: false,
                maximized: false,
            }
        });
        let mut close_clicked = false;
        let mut toggle_min = false;
        let mut toggle_max = false;
        let mut result = None;

        let mut window = egui::Window::new(&self.title)
            .id(self.id)
            .title_bar(false)
            .frame(
                egui::Frame::default()
                    .fill(FACE_GRAY)
                    // egui's own edge/corner resize-drag sensing extends
                    // `style.interact_radius` (5px by default) in from
                    // each edge -- less margin than this and the title
                    // bar's own buttons (particularly the top-right
                    // close button, right where the top and right resize
                    // zones overlap) end up fighting the resize sense for
                    // the same pixels and lose.
                    .inner_margin(egui::Margin::same(6))
                    .stroke(egui::Stroke::NONE)
                    .shadow(egui::Shadow::NONE),
            );

        // Maximized/minimized windows are pinned in place (real OS windows
        // aren't draggable/resizable in either state either).
        //
        // Otherwise, position and size are *both* hand-rolled via
        // `current_pos`/`fixed_size`, not left to egui's own native
        // dragging/resizing, for two independent reasons:
        //  - `title_bar(false)` on a movable window makes egui treat the
        //    *whole body* as a drag-to-move target (there's no title bar
        //    left to grab). That's deliberately a low-priority catch-all
        //    (see egui's own comment on `Window::show_dyn`: it "absorbs
        //    drag hit-tests over the body, so the resize-edge widgets
        //    aren't picked as the closest drag target") -- which also
        //    means it absorbs hit-tests meant for *our* buttons and resize
        //    grip, silently swallowing every click. `.movable(false)`
        //    below turns this off entirely; dragging is instead handled
        //    narrowly, just on the title label below, which -- being an
        //    ordinary nested widget instead of a whole-body catch-all --
        //    doesn't compete with anything else.
        //  - Native resizing has a separate, unrelated problem: it
        //    silently refuses to grow a window's height past whatever its
        //    content naturally needs (measured content height keeps
        //    winning the tug-of-war against a resize drag whenever the
        //    content is shorter than the dragged-to size, snapping the
        //    height right back). `fixed_size` forces the size
        //    unconditionally every frame regardless of content,
        //    sidestepping that auto-shrink entirely; the resize grip
        //    below computes the size it forces.
        if state.maximized {
            window = window.fixed_rect(desktop);
        } else if state.minimized {
            let collapsed =
                egui::Rect::from_min_size(state.rect.min, egui::vec2(state.rect.width(), CHROME_TITLE_BAR_HEIGHT + 6.0));
            window = window.fixed_rect(collapsed);
        } else {
            window = window.movable(false).current_pos(state.rect.min).fixed_size(state.rect.size());
        }

        let mut resize_delta = egui::Vec2::ZERO;
        let mut drag_delta = egui::Vec2::ZERO;

        window.show(ctx, |ui| {
            bevel_box(ui, ui.max_rect(), true);

            egui::Frame::default().fill(TITLE_NAVY).inner_margin(egui::Margin::symmetric(4, 3)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add(egui::Label::new(self.icon));

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Dedicated Unicode window-control glyphs
                        // (U+2716/1F5D6/1F5D7/1F5D5) -- not the more
                        // obvious X/▲/▼, which egui's bundled fonts don't
                        // actually have glyphs for and would render blank.
                        if chrome_button(ui, self.id.with("close_btn"), "\u{2716}").clicked() {
                            close_clicked = true;
                        }
                        let max_label = if state.maximized { "\u{1f5d7}" } else { "\u{1f5d6}" };
                        if chrome_button(ui, self.id.with("max_btn"), max_label).clicked() {
                            toggle_max = true;
                        }
                        if chrome_button(ui, self.id.with("min_btn"), "\u{1f5d5}").clicked() {
                            toggle_min = true;
                        }

                        // Fills whatever horizontal space is left between
                        // the icon and the buttons -- with native
                        // whole-body dragging turned off (see above),
                        // dragging this is the *only* way to move the
                        // window, same as grabbing a real title bar's
                        // blank area.
                        let (_, drag_rect) = ui.allocate_space(ui.available_size());
                        let title_resp = ui.interact(drag_rect, self.id.with("title_drag"), egui::Sense::click_and_drag());
                        ui.painter().text(
                            drag_rect.left_center(),
                            egui::Align2::LEFT_CENTER,
                            &self.title,
                            egui::FontId::default(),
                            HIGHLIGHT_WHITE,
                        );
                        if title_resp.dragged() {
                            drag_delta = title_resp.drag_delta();
                        }
                        if title_resp.double_clicked() {
                            toggle_max = true;
                        }
                    });
                });
            });

            if !state.minimized {
                ui.add_space(2.0);
                result = Some(add_contents(ui));

                if !state.maximized {
                    // From the *outer* window rect we asked for this frame
                    // (`current_pos` + `fixed_size`, set above), not
                    // `ui.max_rect()` -- that's the content area inside the
                    // window's own frame margin, not the true outer edge
                    // the grip needs to sit flush against.
                    let grip_size = egui::vec2(CHROME_RESIZE_GRIP, CHROME_RESIZE_GRIP);
                    let grip_rect = egui::Rect::from_min_size(state.rect.right_bottom() - grip_size, grip_size);
                    let grip = ui.interact(grip_rect, self.id.with("resize_grip"), egui::Sense::drag());
                    if grip.hovered() || grip.dragged() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeNwSe);
                    }
                    if grip.dragged() {
                        resize_delta = grip.drag_delta();
                    }
                    let painter = ui.painter();
                    for i in 1..4 {
                        let offset = i as f32 * 4.0;
                        painter.line_segment(
                            [grip_rect.right_bottom() - egui::vec2(offset, 1.0), grip_rect.right_bottom() - egui::vec2(1.0, offset)],
                            egui::Stroke::new(1.0, SHADOW_GRAY),
                        );
                    }
                }
            }
        });

        if close_clicked {
            *open = false;
        }
        if toggle_min {
            state.minimized = !state.minimized;
            if state.minimized {
                state.maximized = false;
            }
        }
        if toggle_max {
            state.maximized = !state.maximized;
            if state.maximized {
                state.minimized = false;
            }
        }
        if drag_delta != egui::Vec2::ZERO && !state.maximized {
            state.rect = state.rect.translate(drag_delta);
        }
        if resize_delta != egui::Vec2::ZERO {
            state.rect.max += resize_delta;
            state.rect.max = state.rect.max.max(state.rect.min + CHROME_MIN_SIZE);
        }
        ctx.data_mut(|d| d.insert_temp(state_id, state));
        if *open {
            ctx.request_repaint();
        }
        result
    }
}

#[cfg(test)]
mod chrome_tests {
    use super::*;

    fn base_input() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024.0, 768.0))),
            ..Default::default()
        }
    }

    fn with_event(event: egui::Event) -> egui::RawInput {
        egui::RawInput { events: vec![event], ..base_input() }
    }

    fn press(pos: egui::Pos2) -> egui::RawInput {
        with_event(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::default(),
        })
    }

    fn move_to(pos: egui::Pos2) -> egui::RawInput {
        with_event(egui::Event::PointerMoved(pos))
    }

    fn release(pos: egui::Pos2) -> egui::RawInput {
        with_event(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        })
    }

    /// A no-op frame: no new events, same everything. Window-nested
    /// widgets need one of these after *any* pointer-position-changing
    /// event before their own hover/press/click state catches up to it --
    /// egui resolves which layer/widget is "at" a position using the
    /// previous frame's layout, so a widget's response to an event that
    /// arrived this frame is always one frame behind. A real backend hides
    /// this by feeding a genuine OS mouse-move event ahead of every click;
    /// simulating input by hand means inserting the equivalent settle
    /// frame explicitly, every time.
    fn settle(ctx: &egui::Context, id: egui::Id, open: &mut bool) {
        show_frame(ctx, id, open, base_input());
    }

    /// Moves the pointer to `pos` and lets hover settle there.
    fn hover_at(ctx: &egui::Context, id: egui::Id, open: &mut bool, pos: egui::Pos2) {
        show_frame(ctx, id, open, move_to(pos));
        settle(ctx, id, open);
    }

    /// A full click on whatever's at `pos`: hover, press, settle, release.
    fn click_at(ctx: &egui::Context, id: egui::Id, open: &mut bool, pos: egui::Pos2) {
        hover_at(ctx, id, open, pos);
        show_frame(ctx, id, open, press(pos));
        settle(ctx, id, open);
        show_frame(ctx, id, open, release(pos));
    }

    /// A full drag from `from` to `to`: hover, press, settle, move, settle.
    fn drag_from_to(ctx: &egui::Context, id: egui::Id, open: &mut bool, from: egui::Pos2, to: egui::Pos2) {
        hover_at(ctx, id, open, from);
        show_frame(ctx, id, open, press(from));
        settle(ctx, id, open);
        show_frame(ctx, id, open, move_to(to));
        settle(ctx, id, open);
    }

    fn chrome_state(ctx: &egui::Context, id: egui::Id) -> Option<ChromeState> {
        ctx.data(|d| d.get_temp::<ChromeState>(id.with("chrome_state")))
    }

    /// The exact rect of one of the title bar's own explicit-id widgets
    /// (see `chrome_button`/`title_drag`/`resize_grip`) as of the last
    /// frame -- reading this instead of estimating pixel offsets from a
    /// window corner by hand is what makes these tests robust to layout
    /// tweaks (margins, spacing, font metrics) that would otherwise silently
    /// make a guessed click point miss its target and fail for the wrong
    /// reason.
    fn control_rect(ctx: &egui::Context, id: egui::Id, salt: &str) -> egui::Rect {
        let mut found = None;
        let _ = ctx.run_ui(base_input(), |ui| {
            found = ui.ctx().read_response(id.with(salt)).map(|r| r.rect);
        });
        found.unwrap_or_else(|| panic!("no response recorded for {salt}"))
    }

    /// Draws one frame of a `DecoratedWindow` with a fixed id/size, feeding
    /// it `input` -- the same window rebuilt fresh each call, the way a real
    /// app rebuilds its `egui::Window` every frame; only `ChromeState` (kept
    /// in `ctx`'s temp storage) carries anything across these calls, exactly
    /// as it would across real frames. The content block is sized close to
    /// `default_size` -- every real app window here fills its space with
    /// panels/text areas, not a single short label, and a window whose
    /// content collapses well below `CHROME_MIN_SIZE` is a fundamentally
    /// different (and untested) shape than any real caller produces.
    fn show_frame(ctx: &egui::Context, id: egui::Id, open: &mut bool, input: egui::RawInput) {
        let _ = ctx.run_ui(input, |ui| {
            let cctx = ui.ctx().clone();
            DecoratedWindow::new(id, "Test Window").default_size([300.0, 200.0]).show(&cctx, open, |ui| {
                ui.allocate_space(egui::vec2(280.0, 170.0));
            });
        });
    }

    #[test]
    fn dragging_the_title_bar_moves_the_window() {
        let ctx = egui::Context::default();
        let id = egui::Id::new("drag_test_window");
        let mut open = true;

        settle(&ctx, id, &mut open);
        let start_rect = chrome_state(&ctx, id).expect("state after first show").rect;
        let title_point = control_rect(&ctx, id, "title_drag").center();

        let delta = egui::vec2(40.0, 25.0);
        drag_from_to(&ctx, id, &mut open, title_point, title_point + delta);

        let moved_rect = chrome_state(&ctx, id).expect("state after drag").rect;
        assert!(
            (moved_rect.min - (start_rect.min + delta)).length() < 1.0,
            "expected the window to have moved by {delta:?} to {:?}, but it's at {:?} (started at {:?})",
            start_rect.min + delta,
            moved_rect.min,
            start_rect.min,
        );
    }

    #[test]
    fn dragging_the_resize_grip_resizes_the_window() {
        let ctx = egui::Context::default();
        let id = egui::Id::new("resize_test_window");
        let mut open = true;

        settle(&ctx, id, &mut open);
        let start_rect = chrome_state(&ctx, id).expect("state after first show").rect;
        let grip_point = control_rect(&ctx, id, "resize_grip").center();

        let delta = egui::vec2(60.0, 45.0);
        drag_from_to(&ctx, id, &mut open, grip_point, grip_point + delta);

        let resized_rect = chrome_state(&ctx, id).expect("state after resize").rect;
        assert!(
            resized_rect.width() > start_rect.width() + 30.0 && resized_rect.height() > start_rect.height() + 20.0,
            "expected the window to have grown from {:?} past roughly {:?}, but it's now {:?}",
            start_rect.size(),
            start_rect.size() + egui::vec2(30.0, 20.0),
            resized_rect.size(),
        );
        // Resizing shouldn't move the window's top-left corner.
        assert_eq!(resized_rect.min, start_rect.min);
    }

    #[test]
    fn close_button_click_sets_open_to_false() {
        let ctx = egui::Context::default();
        let id = egui::Id::new("close_test_window");
        let mut open = true;

        settle(&ctx, id, &mut open);
        let close_point = control_rect(&ctx, id, "close_btn").center();
        click_at(&ctx, id, &mut open, close_point);

        assert!(!open, "clicking the close button should have set `open` to false");
    }

    #[test]
    fn maximize_then_restore_returns_to_the_pre_maximize_geometry() {
        let ctx = egui::Context::default();
        let id = egui::Id::new("maximize_test_window");
        let mut open = true;

        settle(&ctx, id, &mut open);
        let start_rect = chrome_state(&ctx, id).expect("state after first show").rect;
        let maximize_point = control_rect(&ctx, id, "max_btn").center();
        click_at(&ctx, id, &mut open, maximize_point);

        let maximized = chrome_state(&ctx, id).expect("state after maximize");
        assert!(maximized.maximized, "clicking maximize should have set the maximized flag");
        assert!(
            maximized.rect == start_rect,
            "the restore-to geometry shouldn't change just from maximizing: was {start_rect:?}, now {:?}",
            maximized.rect,
        );

        // The click above flipped the `maximized` flag, but the window's
        // *rendered* geometry for that same release frame was already
        // computed from the flag's pre-click value (geometry is decided
        // at the top of `show`, before that frame's own click is
        // processed) -- one more settle frame lets the window actually
        // redraw at the maximized rect before reading where its buttons
        // ended up.
        settle(&ctx, id, &mut open);

        // Click the same (now "restore") button again -- re-read its rect
        // first, since maximizing moved the whole window (and the button
        // with it) to cover the desktop.
        let restore_point = control_rect(&ctx, id, "max_btn").center();
        click_at(&ctx, id, &mut open, restore_point);

        let restored = chrome_state(&ctx, id).expect("state after restore");
        assert!(!restored.maximized, "clicking restore should have cleared the maximized flag");
        assert_eq!(restored.rect, start_rect, "restoring should return to the exact pre-maximize geometry");
    }

    #[test]
    fn minimize_then_restore_returns_to_the_pre_minimize_geometry() {
        let ctx = egui::Context::default();
        let id = egui::Id::new("minimize_test_window");
        let mut open = true;

        settle(&ctx, id, &mut open);
        let start_rect = chrome_state(&ctx, id).expect("state after first show").rect;
        let minimize_point = control_rect(&ctx, id, "min_btn").center();
        click_at(&ctx, id, &mut open, minimize_point);

        let minimized = chrome_state(&ctx, id).expect("state after minimize");
        assert!(minimized.minimized, "clicking minimize should have set the minimized flag");
        assert_eq!(minimized.rect, start_rect, "the restore-to geometry shouldn't change just from minimizing");

        let restore_point = control_rect(&ctx, id, "min_btn").center();
        click_at(&ctx, id, &mut open, restore_point);

        let restored = chrome_state(&ctx, id).expect("state after restore");
        assert!(!restored.minimized, "clicking restore should have cleared the minimized flag");
        assert_eq!(restored.rect, start_rect, "restoring should return to the exact pre-minimize geometry");
    }
}
