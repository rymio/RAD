//! `rymwin --embed`: the desktop without a window of its own, so RAD can show it inside its
//! main window (the RYMWIN view) the same way it shows its browser engines.
//!
//! egui runs exactly as in the native window, but every frame is rendered off-screen with
//! wgpu and read back as RGBA. The protocol is RAD's browser-engine protocol:
//!   - stdin: one JSON command per line (`resize`, `mouse_move`, `mouse_button`, `wheel`,
//!     `key`, `text`, `copy`, `cut`, `paste`, `focus`, `hover`, `drop`, `mouse_leave`);
//!   - stdout: messages of `[type u8][len u32 LE][payload]`: type 1 is a JSON event
//!     (`ready`, `cursor`, `copy`, `open_url`), type 2 a block of changed frame rows,
//!     `[w u32][h u32][y0 u32][rows u32]` then `rows * w * 4` RGBA bytes.

use std::io::{BufRead, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use eframe::egui;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, Vec2, ViewportId, ViewportInfo};
use serde_json::{Value, json};

const MIN_FRAME: Duration = Duration::from_millis(16);

pub struct Options {
    pub width: u32,
    pub height: u32,
    pub scale: f32,
}

impl Options {
    /// `--width W --height H --scale S` from the command line (device pixels).
    pub fn from_args(args: &[String]) -> Options {
        let get = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1));
        Options {
            width: get("--width").and_then(|v| v.parse().ok()).unwrap_or(1024).max(1),
            height: get("--height").and_then(|v| v.parse().ok()).unwrap_or(768).max(1),
            scale: get("--scale").and_then(|v| v.parse().ok()).filter(|s: &f32| *s > 0.0).unwrap_or(1.0),
        }
    }
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: egui_wgpu::Renderer,
    target: Option<(u32, u32, wgpu::Texture, wgpu::Buffer)>,
}

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

impl Gpu {
    fn new() -> anyhow::Result<Gpu> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            force_fallback_adapter: false,
            compatible_surface: None,
        }))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("rymwin-embed"),
            ..Default::default()
        }))?;
        let renderer = egui_wgpu::Renderer::new(&device, FORMAT, egui_wgpu::RendererOptions::default());
        Ok(Gpu { device, queue, renderer, target: None })
    }

    fn padded_row(w: u32) -> u32 {
        let row = w * 4;
        row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT
    }

    fn ensure_target(&mut self, w: u32, h: u32) {
        if self.target.as_ref().is_some_and(|t| t.0 == w && t.1 == h) {
            return;
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("rymwin-frame"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rymwin-readback"),
            size: (Self::padded_row(w) * h) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.target = Some((w, h, texture, buffer));
    }

    /// Renders one egui frame and returns its pixels, tightly packed RGBA.
    fn render(&mut self, w: u32, h: u32, ppp: f32, prims: &[egui::ClippedPrimitive], textures: &egui::TexturesDelta) -> Vec<u8> {
        self.ensure_target(w, h);
        for (id, delta) in &textures.set {
            self.renderer.update_texture(&self.device, &self.queue, *id, delta);
        }
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: [w, h], pixels_per_point: ppp };
        let (_, _, texture, buffer) = self.target.as_ref().unwrap();
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("rymwin") });
        let extra = self.renderer.update_buffers(&self.device, &self.queue, &mut encoder, prims, &screen);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("rymwin"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let mut pass = pass.forget_lifetime();
            self.renderer.render(&mut pass, prims, &screen);
        }
        let padded = Self::padded_row(w);
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.queue.submit(extra.into_iter().chain(std::iter::once(encoder.finish())));
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        let mut px = Vec::with_capacity((w * h * 4) as usize);
        {
            let data = slice.get_mapped_range();
            for row in 0..h {
                let start = (row * padded) as usize;
                px.extend_from_slice(&data[start..start + (w * 4) as usize]);
            }
        }
        buffer.unmap();
        for id in &textures.free {
            self.renderer.free_texture(id);
        }
        px
    }
}

fn send(out: &mut impl Write, kind: u8, payload: &[u8]) -> std::io::Result<()> {
    out.write_all(&[kind])?;
    out.write_all(&(payload.len() as u32).to_le_bytes())?;
    out.write_all(payload)?;
    out.flush()
}

fn send_event(out: &mut impl Write, v: Value) -> std::io::Result<()> {
    send(out, 1, &serde_json::to_vec(&v).unwrap_or_default())
}

/// Sends the rows that changed since `prev` (all of them after a resize).
fn send_frame(out: &mut impl Write, w: u32, h: u32, px: &[u8], prev: &[u8]) -> std::io::Result<()> {
    let stride = (w * 4) as usize;
    let same = |r: usize| prev.len() == px.len() && px[r * stride..(r + 1) * stride] == prev[r * stride..(r + 1) * stride];
    let Some(first) = (0..h as usize).find(|&r| !same(r)) else { return Ok(()) };
    let last = (first..h as usize).rev().find(|&r| !same(r)).unwrap_or(first);
    let rows = (last - first + 1) as u32;
    let mut msg = Vec::with_capacity(16 + rows as usize * stride);
    for v in [w, h, first as u32, rows] {
        msg.extend_from_slice(&v.to_le_bytes());
    }
    msg.extend_from_slice(&px[first * stride..(last + 1) * stride]);
    send(out, 2, &msg)
}

fn cursor_css(c: egui::CursorIcon) -> &'static str {
    use egui::CursorIcon as C;
    match c {
        C::None => "none",
        C::PointingHand => "pointer",
        C::Text | C::VerticalText => "text",
        C::Crosshair => "crosshair",
        C::Grab => "grab",
        C::Grabbing => "grabbing",
        C::Move | C::AllScroll => "move",
        C::NotAllowed | C::NoDrop => "not-allowed",
        C::Wait => "wait",
        C::Progress => "progress",
        C::Help => "help",
        C::ResizeHorizontal | C::ResizeColumn | C::ResizeEast | C::ResizeWest => "ew-resize",
        C::ResizeVertical | C::ResizeRow | C::ResizeNorth | C::ResizeSouth => "ns-resize",
        C::ResizeNeSw | C::ResizeNorthEast | C::ResizeSouthWest => "nesw-resize",
        C::ResizeNwSe | C::ResizeNorthWest | C::ResizeSouthEast => "nwse-resize",
        C::ZoomIn => "zoom-in",
        C::ZoomOut => "zoom-out",
        _ => "default",
    }
}

/// Turns one command from RAD into egui input; returns false for `quit`.
struct Input {
    w: u32,
    h: u32,
    scale: f32,
    focused: bool,
    modifiers: Modifiers,
    events: Vec<Event>,
    hovering: bool,
    dropped: Vec<egui::DroppedFile>,
}

impl Input {
    fn pos(&self, v: &Value) -> Pos2 {
        Pos2::new(v["x"].as_f64().unwrap_or(0.0) as f32 / self.scale, v["y"].as_f64().unwrap_or(0.0) as f32 / self.scale)
    }

    fn mods(&mut self, v: &Value) -> Modifiers {
        let b = |k: &str| v[k].as_bool().unwrap_or(false);
        let mac = cfg!(target_os = "macos");
        self.modifiers = Modifiers {
            alt: b("alt"),
            ctrl: b("ctrl"),
            shift: b("shift"),
            mac_cmd: mac && b("meta"),
            command: if mac { b("meta") } else { b("ctrl") },
        };
        self.modifiers
    }

    fn apply(&mut self, v: &Value) -> bool {
        match v["cmd"].as_str().unwrap_or("") {
            "quit" => return false,
            "resize" => {
                self.w = v["w"].as_u64().unwrap_or(self.w as u64).max(1) as u32;
                self.h = v["h"].as_u64().unwrap_or(self.h as u64).max(1) as u32;
                if let Some(s) = v["scale"].as_f64().filter(|s| *s > 0.0) {
                    self.scale = s as f32;
                }
            }
            "focus" => {
                self.focused = v["focused"].as_bool().unwrap_or(true);
                self.events.push(Event::WindowFocused(self.focused));
            }
            "mouse_move" => {
                let p = self.pos(v);
                self.events.push(Event::PointerMoved(p));
            }
            "mouse_leave" => self.events.push(Event::PointerGone),
            "mouse_button" => {
                let button = match v["button"].as_u64() {
                    Some(1) => PointerButton::Middle,
                    Some(2) => PointerButton::Secondary,
                    _ => PointerButton::Primary,
                };
                let pos = self.pos(v);
                let modifiers = self.mods(v);
                self.events.push(Event::PointerMoved(pos));
                self.events.push(Event::PointerButton { pos, button, pressed: v["action"] == "down", modifiers });
            }
            "wheel" => {
                let (dx, dy) = (v["dx"].as_f64().unwrap_or(0.0) as f32, v["dy"].as_f64().unwrap_or(0.0) as f32);
                let (unit, delta) = match v["mode"].as_u64() {
                    Some(1) => (egui::MouseWheelUnit::Line, Vec2::new(-dx, -dy)),
                    Some(2) => (egui::MouseWheelUnit::Page, Vec2::new(-dx, -dy)),
                    _ => (egui::MouseWheelUnit::Point, Vec2::new(-dx, -dy) / self.scale),
                };
                let modifiers = self.mods(v);
                self.events.push(Event::PointerMoved(self.pos(v)));
                self.events.push(Event::MouseWheel { unit, delta, phase: egui::TouchPhase::Move, modifiers });
            }
            "key" => {
                let modifiers = self.mods(v);
                let pressed = v["state"] == "down";
                let name = v["key"].as_str().unwrap_or("");
                let code = v["code"].as_str().unwrap_or("");
                let physical = Key::from_name(code.trim_start_matches("Key").trim_start_matches("Digit"));
                if let Some(key) = Key::from_name(name).or(physical) {
                    self.events.push(Event::Key { key, physical_key: physical, pressed, repeat: v["repeat"].as_bool().unwrap_or(false), modifiers });
                }
                // Typing: printable characters without a command modifier become text.
                if pressed && name.chars().count() == 1 && !modifiers.ctrl && !modifiers.mac_cmd {
                    self.events.push(Event::Text(name.to_string()));
                }
            }
            "text" => {
                if let Some(t) = v["text"].as_str() {
                    self.events.push(Event::Text(t.to_string()));
                }
            }
            "copy" => self.events.push(Event::Copy),
            "cut" => self.events.push(Event::Cut),
            "paste" => {
                if let Some(t) = v["text"].as_str() {
                    self.events.push(Event::Paste(t.to_string()));
                }
            }
            "hover" => self.hovering = v["active"].as_bool().unwrap_or(false),
            "drop" => {
                self.hovering = false;
                for p in v["paths"].as_array().into_iter().flatten().filter_map(|p| p.as_str()) {
                    let path = std::path::PathBuf::from(p);
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    self.dropped.push(egui::DroppedFile { path: Some(path), name, ..Default::default() });
                }
            }
            _ => {}
        }
        true
    }
}

/// Runs the desktop off-screen until stdin closes. `setup` runs once with the context (theme,
/// image loaders); `draw` is the same per-frame UI the native window runs.
pub fn run(opts: Options, setup: impl FnOnce(&egui::Context), mut draw: impl FnMut(&mut egui::Ui)) -> anyhow::Result<()> {
    let mut gpu = Gpu::new()?;
    let ctx = egui::Context::default();
    setup(&ctx);

    let (tx, rx) = mpsc::channel::<Value>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                if tx.send(v).is_err() {
                    break;
                }
            }
        }
    });

    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::with_capacity(1 << 20, stdout.lock());
    send_event(&mut out, json!({"type": "ready"}))?;

    let start = Instant::now();
    let max_side = gpu.device.limits().max_texture_dimension_2d as usize;
    let mut input = Input {
        w: opts.width,
        h: opts.height,
        scale: opts.scale,
        focused: true,
        modifiers: Modifiers::default(),
        events: Vec::new(),
        hovering: false,
        dropped: Vec::new(),
    };
    let mut prev: Vec<u8> = Vec::new();
    let mut prev_size = (0, 0);
    let mut cursor = "";
    let mut next = Instant::now();
    let mut last_frame = Instant::now() - MIN_FRAME;

    loop {
        // Wait for input or the next repaint egui asked for, whichever comes first.
        let wait = next.saturating_duration_since(Instant::now());
        match rx.recv_timeout(wait) {
            Ok(v) => {
                if !input.apply(&v) {
                    return Ok(());
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        while let Ok(v) = rx.try_recv() {
            if !input.apply(&v) {
                return Ok(());
            }
        }
        // At most ~60 frames a second; input arriving faster is batched.
        let since = last_frame.elapsed();
        if since < MIN_FRAME {
            std::thread::sleep(MIN_FRAME - since);
            while let Ok(v) = rx.try_recv() {
                if !input.apply(&v) {
                    return Ok(());
                }
            }
        }
        last_frame = Instant::now();

        let (w, h, scale) = (input.w, input.h, input.scale);
        let size = Vec2::new(w as f32, h as f32) / scale;
        let mut raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            max_texture_side: Some(max_side),
            time: Some(start.elapsed().as_secs_f64()),
            modifiers: input.modifiers,
            events: std::mem::take(&mut input.events),
            focused: input.focused,
            dropped_files: std::mem::take(&mut input.dropped),
            ..Default::default()
        };
        if input.hovering {
            raw.hovered_files.push(egui::HoveredFile::default());
        }
        raw.viewports.insert(
            ViewportId::ROOT,
            ViewportInfo {
                native_pixels_per_point: Some(scale),
                inner_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                focused: Some(input.focused),
                ..Default::default()
            },
        );

        let output = ctx.run_ui(raw, |ui| draw(ui));
        let prims = ctx.tessellate(output.shapes, output.pixels_per_point);
        let px = gpu.render(w, h, output.pixels_per_point, &prims, &output.textures_delta);
        if prev_size != (w, h) {
            prev.clear();
            prev_size = (w, h);
        }
        send_frame(&mut out, w, h, &px, &prev)?;
        prev = px;

        let c = cursor_css(output.platform_output.cursor_icon);
        if c != cursor {
            cursor = c;
            send_event(&mut out, json!({"type": "cursor", "cursor": c}))?;
        }
        for cmd in output.platform_output.commands {
            match cmd {
                egui::OutputCommand::CopyText(text) => send_event(&mut out, json!({"type": "copy", "text": text}))?,
                egui::OutputCommand::OpenUrl(u) => send_event(&mut out, json!({"type": "open_url", "url": u.url}))?,
                egui::OutputCommand::CopyImage(_) => {}
            }
        }

        let delay = output.viewport_output.get(&ViewportId::ROOT).map(|v| v.repaint_delay).unwrap_or(Duration::MAX);
        // Nothing scheduled: sleep until input, but wake once a second for the clock.
        next = Instant::now() + delay.min(Duration::from_secs(1));
    }
}
